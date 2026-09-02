//! Contextual display fallbacks for a provider-owned current thread name.

/// Whether an exact metadata read observed a usable native thread name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NameState {
    Named,
    KnownEmpty,
    Unavailable,
}

/// Context retained only for presentation while the current tip lacks a name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NameContext {
    Normal,
    Starting,
}

/// Derived display text and provenance; neither is a persisted user-authored label.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveName {
    pub text: String,
    pub source: EffectiveNameSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectiveNameSource {
    Native,
    CachedStale,
    Synthetic,
}

/// Resolves a current-tip display without writing to a provider or inventing a label.
#[must_use]
pub fn resolve_name(
    state: NameState,
    native_name: Option<&str>,
    cached_name: Option<&str>,
    context: NameContext,
) -> EffectiveName {
    if state == NameState::Named
        && let Some(name) = native_name.filter(|name| !name.trim().is_empty())
    {
        return EffectiveName {
            text: name.to_owned(),
            source: EffectiveNameSource::Native,
        };
    }
    if state == NameState::Unavailable
        && let Some(name) = cached_name.filter(|name| !name.trim().is_empty())
    {
        return EffectiveName {
            text: format!("{name} · stale"),
            source: EffectiveNameSource::CachedStale,
        };
    }
    match context {
        NameContext::Starting => EffectiveName {
            text: "starting".to_owned(),
            source: EffectiveNameSource::Synthetic,
        },
        NameContext::Normal if state == NameState::Unavailable => EffectiveName {
            text: "name unavailable".to_owned(),
            source: EffectiveNameSource::Synthetic,
        },
        NameContext::Normal => EffectiveName {
            text: "untitled".to_owned(),
            source: EffectiveNameSource::Synthetic,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_name_always_wins() {
        assert_eq!(
            resolve_name(NameState::Named, Some("Native"), None, NameContext::Normal,),
            EffectiveName {
                text: "Native".to_owned(),
                source: EffectiveNameSource::Native
            }
        );
    }

    #[test]
    fn unavailable_is_not_mistaken_for_known_empty() {
        let name = resolve_name(
            NameState::Unavailable,
            None,
            Some("Cached"),
            NameContext::Normal,
        );
        assert_eq!(name.text, "Cached · stale");
        assert_eq!(name.source, EffectiveNameSource::CachedStale);
    }

    #[test]
    fn synthetic_fallbacks_do_not_expose_internal_identifiers() {
        assert_eq!(
            resolve_name(NameState::KnownEmpty, None, None, NameContext::Normal).text,
            "untitled"
        );
        assert_eq!(
            resolve_name(NameState::KnownEmpty, None, None, NameContext::Starting).text,
            "starting"
        );
    }
}
