//! System location is reduced to a country on the device. Coordinates never enter chat state.

use std::io::Cursor;
use std::sync::{OnceLock, mpsc::Sender};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebRegionPrompt {
    Ask,
    Locate,
    ChooseCountry,
}

pub(crate) const SUPPORTED: bool = cfg!(any(windows, target_os = "linux", target_os = "android"));

type Point = [f64; 2];
type Ring = Vec<Point>;
type Polygon = Vec<Ring>;
type Borders = Vec<(String, Vec<Polygon>)>;

fn borders() -> Option<&'static Borders> {
    static COUNTRY_BORDERS: OnceLock<Option<Borders>> = OnceLock::new();
    COUNTRY_BORDERS.get_or_init(|| {
        let mut archive = zip::ZipArchive::new(Cursor::new(include_bytes!("../assets/country-boundaries.zip"))).ok()?;
        serde_json::from_reader(archive.by_name("countries.json").ok()?).ok()
    }).as_ref()
}

fn inside([longitude, latitude]: Point, ring: &Ring) -> bool {
    let mut covered = false;
    for edge in ring.windows(2) {
        let [x1, y1] = edge[0];
        let [x2, y2] = edge[1];
        if (y1 > latitude) != (y2 > latitude)
            && longitude < x1 + (latitude - y1) * (x2 - x1) / (y2 - y1)
        {
            covered = !covered;
        }
    }
    covered
}

fn country_at(point: Point) -> Option<&'static str> {
    let mut matches = borders()?.iter().filter(|(_, polygons)| polygons.iter().any(|polygon| {
        polygon.first().is_some_and(|outside| inside(point, outside))
            && !polygon.iter().skip(1).any(|hole| inside(point, hole))
    })).map(|(country, _)| country.as_str());
    let country = matches.next()?;
    matches.all(|other| other == country).then_some(country)
}

fn resolve_country(latitude: f64, longitude: f64, accuracy: f64) -> Option<String> {
    if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude)
        || !(0.0..=200_000.0).contains(&accuracy)
    {
        return None;
    }
    let country = country_at([longitude, latitude])?;
    // If the reported uncertainty reaches another country, ask for a market instead.
    let degrees_lat = accuracy / 111_000.0;
    let degrees_lon = degrees_lat / latitude.to_radians().cos().max(0.05);
    for bearing in 0..8 {
        let angle = (bearing as f64) * std::f64::consts::FRAC_PI_4;
        let lon = (longitude + degrees_lon * angle.cos() + 180.0).rem_euclid(360.0) - 180.0;
        let lat = latitude + degrees_lat * angle.sin();
        if country_at([lon, lat]).is_some_and(|nearby| nearby != country) {
            return None;
        }
    }
    crate::browser::country_code(country)
}

/// Starts from the foreground UI after the user allows location access.
pub(crate) fn request(id: u64, result: Sender<Option<String>>) {
    #[cfg(windows)]
    {
        let _ = id;
        use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator, PositionAccuracy};
        use windows::Foundation::TimeSpan;
        use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

        // Windows requires the access request to originate on the foreground UI thread.
        let Ok(access) = Geolocator::RequestAccessAsync() else {
            let _ = result.send(None);
            return;
        };
        std::thread::spawn(move || {
            // This dedicated thread owns the matching COM uninitialization call.
            let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_ok();
            let country = initialized.then(|| {
                if access.get().ok()? != GeolocationAccessStatus::Allowed { return None; }
                let locator = Geolocator::new().ok()?;
                locator.SetDesiredAccuracy(PositionAccuracy::Default).ok()?;
                let position = locator.GetGeopositionAsyncWithAgeAndTimeout(
                    TimeSpan { Duration: 60 * 10_000_000 }, TimeSpan { Duration: 12 * 10_000_000 },
                ).ok()?.get().ok()?;
                let coordinate = position.Coordinate().ok()?;
                let point = coordinate.Point().ok()?.Position().ok()?;
                resolve_country(point.Latitude, point.Longitude, coordinate.Accuracy().ok()?)
            }).flatten();
            if initialized { unsafe { RoUninitialize() }; }
            let _ = result.send(country);
        });
    }
    #[cfg(target_os = "linux")]
    std::thread::spawn(move || {
        let country = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()
            .and_then(|runtime| runtime.block_on(linux_location(id)));
        let _ = result.send(country);
    });
    #[cfg(target_os = "android")]
    {
        *ANDROID_REQUEST.lock().unwrap() = Some((id, result));
        if !crate::android::request_web_location(id) {
            finish_android(id, None);
        }
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
    { let _ = (id, result.send(None)); }
}

#[cfg(target_os = "linux")]
async fn linux_location(id: u64) -> Option<String> {
    use futures_util::StreamExt;
    use std::{collections::HashMap, time::Duration};
    use zbus::{Connection, Proxy, zvariant::{OwnedObjectPath, OwnedValue, Value}};

    let connection = tokio::time::timeout(Duration::from_secs(3), Connection::session()).await.ok()?.ok()?;
    let desktop = "org.freedesktop.portal.Desktop";
    let portal = Proxy::new(&connection, desktop, "/org/freedesktop/portal/desktop", "org.freedesktop.portal.Location").await.ok()?;
    let token = format!("spindle_{}_{id}", std::process::id());
    let sender = connection.unique_name()?.as_str().trim_start_matches(':').replace('.', "_");
    let path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let request = Proxy::new(&connection, desktop, path.as_str(), "org.freedesktop.portal.Request").await.ok()?;
    let mut responses = request.receive_signal("Response").await.ok()?;
    let mut locations = portal.receive_signal("LocationUpdated").await.ok()?;
    let options = HashMap::from([("session_handle_token", Value::from(token.as_str())), ("accuracy", Value::from(1u32))]);
    let session: OwnedObjectPath = tokio::time::timeout(Duration::from_secs(3), portal.call("CreateSession", &(options,))).await.ok()?.ok()?;
    let position = tokio::time::timeout(Duration::from_secs(25), async {
        let options = HashMap::from([("handle_token", Value::from(token.as_str()))]);
        let _: OwnedObjectPath = portal.call("Start", &(&session, "", options)).await.ok()?;
        let (code, _): (u32, HashMap<String, OwnedValue>) = responses.next().await?.body().deserialize().ok()?;
        if code != 0 { return None; }
        while let Some(signal) = locations.next().await {
            let (handle, mut data): (OwnedObjectPath, HashMap<String, OwnedValue>) = signal.body().deserialize().ok()?;
            if handle == session {
                return resolve_country(f64::try_from(data.remove("Latitude")?).ok()?,
                    f64::try_from(data.remove("Longitude")?).ok()?, f64::try_from(data.remove("Accuracy")?).ok()?);
            }
        }
        None
    }).await.ok().flatten();
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        let session = Proxy::new(&connection, desktop, session.as_str(), "org.freedesktop.portal.Session").await?;
        session.call::<_, _, ()>("Close", &()).await
    }).await;
    position
}

#[cfg(target_os = "android")]
static ANDROID_REQUEST: std::sync::Mutex<Option<(u64, Sender<Option<String>>)>> = std::sync::Mutex::new(None);

#[cfg(target_os = "android")]
fn finish_android(id: u64, country: Option<String>) {
    let mut request = ANDROID_REQUEST.lock().unwrap();
    if request.as_ref().is_some_and(|(pending, _)| *pending == id)
        && let Some((_, result)) = request.take()
    {
        let _ = result.send(country);
    }
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_spindleapp_spindle_SpindleActivity_nativeWebLocation(
    _env: jni::JNIEnv, _activity: jni::objects::JObject, id: jni::sys::jlong,
    latitude: f64, longitude: f64, accuracy: f64,
) {
    // The polygon lookup is CPU work, so leave Android's UI thread immediately.
    std::thread::spawn(move || finish_android(id as u64, resolve_country(latitude, longitude, accuracy)));
}

#[cfg(test)]
mod tests {
    use super::resolve_country;

    #[test]
    fn coordinates_resolve_locally_and_uncertain_fixes_are_ignored() {
        assert_eq!(resolve_country(39.74, -104.99, 1000.0).as_deref(), Some("US"));
        assert_eq!(resolve_country(51.50, -0.12, 1000.0).as_deref(), Some("GB"));
        assert_eq!(resolve_country(43.653, -79.383, 1000.0).as_deref(), Some("CA"));
        assert_eq!(resolve_country(42.32, -83.05, 20000.0), None);
        assert_eq!(resolve_country(0.0, 0.0, 1000.0), None);
        assert_eq!(resolve_country(39.74, -104.99, 1_000_000.0), None);
    }
}
