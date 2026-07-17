//! Per-word autocomplete acceptance counters.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::layout::Lang;

/// Locally learned acceptance counts, separated by keyboard language.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WordUsage {
    en: BTreeMap<String, u32>,
    ru: BTreeMap<String, u32>,
}

impl WordUsage {
    fn map(&self, lang: Lang) -> &BTreeMap<String, u32> {
        match lang {
            Lang::En => &self.en,
            Lang::Ru => &self.ru,
        }
    }

    fn map_mut(&mut self, lang: Lang) -> &mut BTreeMap<String, u32> {
        match lang {
            Lang::En => &mut self.en,
            Lang::Ru => &mut self.ru,
        }
    }

    /// Returns the learned acceptance count for `word`.
    pub fn count(&self, word: &str, lang: Lang) -> u32 {
        self.map(lang).get(word).copied().unwrap_or(0)
    }

    /// Increments `word` without overflowing and returns its new count.
    pub fn increment(&mut self, word: &str, lang: Lang) -> u32 {
        let map = self.map_mut(lang);
        if let Some(count) = map.get_mut(word) {
            *count = count.saturating_add(1);
            *count
        } else {
            map.insert(word.to_owned(), 1);
            1
        }
    }

    /// Merges the greater count for every word and returns the number changed.
    pub fn merge_max(&mut self, other: &Self) -> usize {
        let mut changed = 0;
        for (lang, word, incoming) in other.iter() {
            if incoming == 0 {
                continue;
            }

            let map = self.map_mut(lang);
            if let Some(current) = map.get_mut(word) {
                if incoming > *current {
                    *current = incoming;
                    changed += 1;
                }
            } else {
                map.insert(word.to_owned(), incoming);
                changed += 1;
            }
        }
        changed
    }

    /// Iterates one language in deterministic word order.
    pub fn iter_lang(&self, lang: Lang) -> impl Iterator<Item = (&str, u32)> + '_ {
        self.map(lang)
            .iter()
            .map(|(word, count)| (word.as_str(), *count))
    }

    /// Iterates both languages in deterministic language and word order.
    pub fn iter(&self) -> impl Iterator<Item = (Lang, &str, u32)> + '_ {
        self.iter_lang(Lang::En)
            .map(|(word, count)| (Lang::En, word, count))
            .chain(
                self.iter_lang(Lang::Ru)
                    .map(|(word, count)| (Lang::Ru, word, count)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increment_is_language_specific_and_saturates() {
        let mut usage = WordUsage::default();
        assert_eq!(usage.increment("ready", Lang::En), 1);
        assert_eq!(usage.increment("ready", Lang::En), 2);
        assert_eq!(usage.count("ready", Lang::Ru), 0);

        usage.en.insert("full".to_owned(), u32::MAX);
        assert_eq!(usage.increment("full", Lang::En), u32::MAX);
        assert_eq!(usage.count("full", Lang::En), u32::MAX);
    }

    #[test]
    fn merge_max_changes_only_lower_counts_and_ignores_zero() {
        let mut local = WordUsage::default();
        local.en.insert("ready".to_owned(), 5);
        local.ru.insert("готово".to_owned(), 2);

        let mut imported = WordUsage::default();
        imported.en.insert("ready".to_owned(), 3);
        imported.en.insert("work".to_owned(), 4);
        imported.ru.insert("готово".to_owned(), 7);
        imported.ru.insert("ноль".to_owned(), 0);

        assert_eq!(local.merge_max(&imported), 2);
        assert_eq!(local.count("ready", Lang::En), 5);
        assert_eq!(local.count("work", Lang::En), 4);
        assert_eq!(local.count("готово", Lang::Ru), 7);
        assert_eq!(local.count("ноль", Lang::Ru), 0);
        assert!(!local.ru.contains_key("ноль"));
    }

    #[test]
    fn iteration_is_deterministic_by_language_then_word() {
        let mut usage = WordUsage::default();
        usage.increment("zebra", Lang::En);
        usage.increment("alpha", Lang::En);
        usage.increment("я", Lang::Ru);
        usage.increment("а", Lang::Ru);

        assert_eq!(
            usage.iter().collect::<Vec<_>>(),
            vec![
                (Lang::En, "alpha", 1),
                (Lang::En, "zebra", 1),
                (Lang::Ru, "а", 1),
                (Lang::Ru, "я", 1),
            ]
        );
    }
}
