use serde::{Deserialize, Serialize};

macro_rules! choices {
    ($name:ident, $first:ident => $first_label:literal, $($variant:ident => $label:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            #[default]
            $first,
            $($variant),+
        }
        impl $name {
            pub const CHOICES: &[(Self, &str)] = &[
                (Self::$first, $first_label), $((Self::$variant, $label)),+
            ];
        }
    };
}

choices!(Role, General => "General", Research => "Research", Build => "Build", Creative => "Creative", Review => "Review", Operations => "Operations");
choices!(Rank, Specialist => "Specialist", Lead => "Lead", Supervisor => "Supervisor", Routine => "Routine worker", Custom => "Custom");
choices!(Outline, Hexagon => "Hexagon", Circle => "Circle", Square => "Square", Octagon => "Octagon", Diamond => "Diamond");
choices!(Kernel, Geometry => "Geometry", Iris => "Shutter iris", Wave => "Radial wave", Star => "Four-point star", Horizon => "Event horizon", Ribbon => "Infinity ribbon", Blob => "Blob", Mirror => "Mirrored waves");
choices!(Architecture, Unknown => "Architecture undisclosed", Dense => "Dense", Moe => "Mixture of experts");
choices!(Terms, Unknown => "Terms unknown", Closed => "Closed or API-only", Restricted => "Restricted weights", Permissive => "Permissive weights");
choices!(Line, Solid => "Solid", Dotted => "Dotted", Dashed => "Dashed", Glow => "Glowing");
choices!(Ticks, Bars => "Lines", Circles => "Hollow circles", Dots => "Dots", Wedges => "Wedges", Chevrons => "Chevrons", Pairs => "Paired bars", Diamonds => "Diamonds");
choices!(Direction, Seeded => "Seeded", Clockwise => "Clockwise", Counterclockwise => "Counterclockwise");
choices!(Shell, Pebble => "Pebble", Cloud => "Cloud", Sprout => "Sprout", Mantle => "Mantle", Kite => "Kite", Lantern => "Lantern", Skiff => "Skiff");

impl Rank {
    pub fn circles(self) -> usize {
        match self {
            Self::Lead => 1,
            Self::Supervisor => 2,
            Self::Specialist => 3,
            Self::Routine => 4,
            Self::Custom => 5,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelMark {
    pub kernel: Kernel,
    pub architecture: Architecture,
    pub terms: Terms,
    pub tier: Option<u8>,
    pub physical_layers: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lineage {
    pub parent: String,
    pub depth: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sigil {
    pub version: u8,
    pub seed: u64,
    pub motion_seed: u64,
    pub role: Role,
    pub rank: Rank,
    pub border: Outline,
    pub geometry: Outline,
    pub ticks: Ticks,
    pub lines: [Line; 6],
    pub directions: [Direction; 6],
    pub shell: Shell,
    pub motion: bool,
    pub lineage: Option<Lineage>,
    pub models: std::collections::BTreeMap<String, ModelMark>,
}

impl Default for Sigil {
    fn default() -> Self {
        Self {
            version: 1,
            seed: 0,
            motion_seed: 0,
            role: Role::default(),
            rank: Rank::default(),
            border: Outline::Hexagon,
            geometry: Outline::Circle,
            ticks: Ticks::default(),
            lines: [
                Line::Solid,
                Line::Dotted,
                Line::Dashed,
                Line::Glow,
                Line::Solid,
                Line::Dashed,
            ],
            directions: [Direction::Seeded; 6],
            shell: Shell::default(),
            motion: false,
            lineage: None,
            models: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TierBasis {
    Reported(f64),
    Override,
    Unknown,
}

impl Sigil {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn new_seed() -> anyhow::Result<u64> {
        Ok(u64::from_le_bytes(crate::vault::random::<8>()?).max(1))
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        use anyhow::ensure;
        ensure!(
            self.version == 1,
            "This sigil recipe needs a compatible Spindle version."
        );
        ensure!(
            self.geometry != Outline::Diamond,
            "Choose circle, square, hexagon, or octagon for the layers."
        );
        ensure!(
            self.models.len() <= 128
                && self.models.iter().all(|(name, mark)| {
                    super::valid_text(name, 240, true)
                        && mark.tier.is_none_or(|tier| tier <= 6)
                        && mark.physical_layers.is_none_or(|layers| layers > 0)
                }),
            "The sigil contains invalid model details."
        );
        Ok(())
    }

    pub fn tier(&self, model: &str, reported: Option<f64>) -> (Option<u8>, TierBasis) {
        if let Some(tier) = self.models.get(model).and_then(|mark| mark.tier) {
            return (Some(tier), TierBasis::Override);
        }
        let Some(size) = reported.filter(|size| size.is_finite() && *size > 0.) else {
            return (None, TierBasis::Unknown);
        };
        // Version 1 uses the parameter anchors from the visual study as lower bounds.
        let tier = [32., 128., 600., 1200., 5000.]
            .into_iter()
            .filter(|anchor| size >= *anchor)
            .count() as u8;
        (Some(tier), TierBasis::Reported(size))
    }

    pub fn mark(&self, model: &str) -> ModelMark {
        self.models.get(model).cloned().unwrap_or_default()
    }

    pub fn lineage_variant(&mut self, parent: String) -> anyhow::Result<()> {
        self.motion_seed = Self::new_seed()?;
        self.lineage = Some(Lineage {
            parent,
            depth: self
                .lineage
                .as_ref()
                .map_or(1, |lineage| lineage.depth.saturating_add(1)),
        });
        Ok(())
    }
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DuplicateAppearance {
    #[default]
    Keep,
    Lineage,
    New,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_changes_keep_identity_and_distinguish_zero_from_unknown() {
        let mut sigil = Sigil {
            seed: 24,
            ..Default::default()
        };
        let original = sigil.clone();
        assert_eq!(sigil.tier("other", None), (None, TierBasis::Unknown));
        assert_eq!(
            sigil.tier("other", Some(9.)),
            (Some(0), TierBasis::Reported(9.))
        );
        assert_eq!(
            sigil.tier("other", Some(128.)),
            (Some(2), TierBasis::Reported(128.))
        );
        sigil.models.insert(
            "one".into(),
            ModelMark {
                tier: Some(6),
                architecture: Architecture::Moe,
                ..Default::default()
            },
        );
        assert_eq!(sigil.tier("one", None), (Some(6), TierBasis::Override));
        assert_eq!(sigil.mark("other").architecture, Architecture::Unknown);
        assert_eq!(sigil.seed, original.seed);
        sigil.lineage_variant("parent".into()).unwrap();
        assert_eq!(sigil.seed, original.seed);
        assert_ne!(sigil.motion_seed, original.motion_seed);
        assert_eq!(sigil.lineage.as_ref().unwrap().depth, 1);
        let restored: Sigil =
            serde_json::from_value(serde_json::to_value(&sigil).unwrap()).unwrap();
        assert_eq!(restored, sigil);
    }
}
