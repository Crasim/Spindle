//! Opt-in, credential-free model connections. Wire addresses always belong to the sender.

use super::adapter::{SyncState, provider_from_wire, provider_value, put_item};
use super::core::{Kind, hex, sha256};
use super::space::Space;
use crate::state::State;
use anyhow::Result;
use reqwest::Url;
use serde_json::json;
use std::net::IpAddr;

const PREFIX: &str = "model-endpoints:";

fn loopback(host: &str) -> bool {
    matches!(
        host.trim_end_matches('.').to_ascii_lowercase().as_str(),
        "localhost" | "localhost.localdomain"
    ) || host
        .trim_matches(['[', ']'])
        .parse::<IpAddr>()
        .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified())
}

pub(super) fn valid_hostname(value: &str) -> bool {
    let value = value.trim_end_matches('.');
    !value.is_empty()
        && value.len() <= 253
        && !loopback(value)
        && value.parse::<IpAddr>().is_err()
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

fn address(base: &str, sender: Option<&str>) -> Option<String> {
    let mut url = Url::parse(base).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    if loopback(url.host_str()?) {
        let host = sender.filter(|host| valid_hostname(host))?;
        url.set_host(Some(host.trim_end_matches('.'))).ok()?;
    }
    Some(url.as_str().trim_end_matches('/').to_owned())
}

pub(super) fn export(state: &State, sync: &mut SyncState, space: &mut Space) -> Result<usize> {
    if !sync.switches.local_providers {
        return Ok(0);
    }
    let system_host = super::live::hostname();
    let host = if sync.provider_host.is_empty() {
        &system_host
    } else {
        &sync.provider_host
    };
    let local_ollama = &state.preferences.ollama;
    let imported_ollama = sync
        .imported_ollama
        .as_ref()
        .is_some_and(|(_, url)| url == local_ollama);
    let ollama = (state.preferences.ollama_on && !imported_ollama)
        .then(|| {
            address(
                if local_ollama.trim().is_empty() {
                    "http://localhost:11434"
                } else {
                    local_ollama
                },
                Some(host),
            )
        })
        .flatten();
    let endpoints: Vec<_> = state
        .providers
        .iter()
        .filter_map(|p| {
            if p.protocol.oauth()
                || p.has_saved_key()
                || p.needs_auth
                || sync.imported_providers.contains_key(&p.id)
            {
                return None;
            }
            let mut wire = provider_value(p);
            wire["base"] = json!(address(&p.base, Some(host))?);
            Some(wire)
        })
        .collect();
    let id = format!("{PREFIX}{}", sync.identity.device.id);
    put_item(
        sync,
        space,
        Kind::Settings,
        &id,
        json!({"ollama": ollama, "providers": endpoints}),
    )
}

pub(super) fn import(state: &mut State, sync: &mut SyncState, space: &Space) -> usize {
    if !sync.switches.local_providers {
        return 0;
    }
    let mut imported = 0;
    for (id, _) in space.items(Kind::Settings) {
        let Some(source) = id.strip_prefix(PREFIX) else {
            continue;
        };
        if source == sync.identity.device.id {
            continue;
        }
        // This namespace is per device, so a different member cannot redirect an existing slot.
        let Some((_, _, record)) = space
            .item(Kind::Settings, &id)
            .into_iter()
            .rev()
            .find(|(_, e, _)| e.device == source)
        else {
            continue;
        };
        if record.deleted {
            continue;
        }
        let record = &record.value;
        let prefs = &mut state.preferences;
        let empty = !prefs.ollama_on && prefs.ollama.is_empty();
        let inherited = prefs.ollama_on
            && sync
                .imported_ollama
                .as_ref()
                .is_some_and(|(from, url)| from == source && url == &prefs.ollama);
        if (empty || inherited)
            && let Some(base) = record["ollama"].as_str().and_then(|u| address(u, None))
            && (base != prefs.ollama || !prefs.ollama_on)
        {
            prefs.ollama = base.clone();
            prefs.ollama_on = true;
            sync.imported_ollama = Some((source.to_owned(), base));
            imported += 1;
        }
        for wire in record["providers"].as_array().into_iter().flatten() {
            let Some(original) = wire["id"].as_str() else {
                continue;
            };
            let Some(mut p) = provider_from_wire(original, wire) else {
                continue;
            };
            if p.protocol.oauth() || p.needs_auth {
                continue;
            }
            let Some(base) = address(&p.base, None) else {
                continue;
            };
            p.base = base;
            let owner = (source.to_owned(), original.to_owned());
            if let Some(existing) = state
                .providers
                .iter_mut()
                .find(|p| sync.imported_providers.get(&p.id) == Some(&owner))
            {
                if !existing.needs_auth && !existing.has_saved_key() && !existing.protocol.oauth() {
                    if !existing.same_connection(&p) {
                        p.id = existing.id.clone();
                        *existing = p;
                        imported += 1;
                    } else {
                        existing.merge_catalog(p);
                    }
                }
                continue;
            }
            if state.providers.iter().any(|local| local.id == original) {
                if state
                    .providers
                    .iter()
                    .any(|local| local.id == original && local.same_connection(&p))
                {
                    continue;
                }
                p.id = format!(
                    "p-{}",
                    hex(&sha256(format!("{source}:{original}").as_bytes())[..16])
                );
                if state.providers.iter().any(|local| local.id == p.id) {
                    continue;
                }
            }
            sync.imported_providers.insert(p.id.clone(), owner);
            state.providers.push(p);
            imported += 1;
        }
    }
    imported
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_loopback_and_keeps_the_port_and_path() {
        for input in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:11434/v1",
            "http://[::1]:11434/v1",
        ] {
            assert_eq!(
                address(input, Some("MODEL-HOST")).as_deref(),
                Some("http://model-host:11434/v1")
            );
            assert_eq!(address(input, None), None);
        }
        assert_eq!(
            address("http://localhost:11434", Some("Phone with spaces")),
            None
        );
        assert_eq!(
            address("https://user:secret@example.org/v1", Some("pc")),
            None
        );
        assert_eq!(
            address("http://ollama.lan:11434", Some("pc")).as_deref(),
            Some("http://ollama.lan:11434")
        );
    }
}
