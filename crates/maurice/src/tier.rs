use std::fmt;

/// Safety tier selected by the binary name (ADR-007).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Read and reversible apply. Refuses destructive plans.
    Marik,
    /// Everything; destructive plans require confirmation.
    Maurice,
    /// Everything without prompts; requires `--yes-i-mean-it` or `MAURICE_FORCE=1`.
    OdysseusStella,
}

impl Tier {
    pub fn from_argv0() -> Self {
        let argv0 = std::env::args().next().unwrap_or_default();
        let name = std::path::Path::new(&argv0)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("maurice");
        Self::from_name(name)
    }

    pub fn from_name(name: &str) -> Self {
        match name {
            "marik" => Tier::Marik,
            "maurice-odysseus-stella" => Tier::OdysseusStella,
            _ => Tier::Maurice,
        }
    }

    pub fn require_at_least(self, min: Tier) -> anyhow::Result<()> {
        if self >= min {
            Ok(())
        } else {
            anyhow::bail!("this action needs the `{min}` tier; you are running as `{self}`")
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Tier::Marik => "marik",
            Tier::Maurice => "maurice",
            Tier::OdysseusStella => "maurice-odysseus-stella",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map_to_tiers() {
        assert_eq!(Tier::from_name("marik"), Tier::Marik);
        assert_eq!(Tier::from_name("maurice"), Tier::Maurice);
        assert_eq!(
            Tier::from_name("maurice-odysseus-stella"),
            Tier::OdysseusStella
        );
        assert_eq!(Tier::from_name("something-else"), Tier::Maurice);
    }

    #[test]
    fn ordering_is_by_power() {
        assert!(Tier::Marik < Tier::Maurice);
        assert!(Tier::Maurice < Tier::OdysseusStella);
        assert!(Tier::Marik.require_at_least(Tier::Maurice).is_err());
        assert!(Tier::OdysseusStella.require_at_least(Tier::Maurice).is_ok());
    }
}
