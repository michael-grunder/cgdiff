use anyhow::{Result, bail};
use memchr::{memchr_iter, memchr2_iter};
use regex::{Regex, RegexBuilder};

trait FilterMatcher {
    fn matches(&self, candidate: &str) -> bool;
}

#[derive(Debug)]
pub(crate) struct SubstringFilter {
    needle: Vec<u8>,
    first_lower: u8,
    first_upper: u8,
}

impl SubstringFilter {
    fn new(query: &str) -> Self {
        let needle = query.as_bytes().to_vec();
        let first = needle[0];
        Self {
            needle,
            first_lower: first.to_ascii_lowercase(),
            first_upper: first.to_ascii_uppercase(),
        }
    }
}

impl FilterMatcher for SubstringFilter {
    fn matches(&self, candidate: &str) -> bool {
        let candidate = candidate.as_bytes();
        if self.needle.len() > candidate.len() {
            return false;
        }

        let candidates = if self.first_lower == self.first_upper {
            EitherMemchrIter::Single(memchr_iter(self.first_lower, candidate))
        } else {
            EitherMemchrIter::Dual(memchr2_iter(
                self.first_lower,
                self.first_upper,
                candidate,
            ))
        };

        for index in candidates {
            let Some(window) = candidate.get(index..index + self.needle.len())
            else {
                break;
            };
            if window.eq_ignore_ascii_case(&self.needle) {
                return true;
            }
        }

        false
    }
}

#[derive(Debug)]
pub(crate) struct RegexFilter {
    regex: Regex,
}

impl FilterMatcher for RegexFilter {
    fn matches(&self, candidate: &str) -> bool {
        self.regex.is_match(candidate)
    }
}

enum EitherMemchrIter<'a> {
    Single(memchr::Memchr<'a>),
    Dual(memchr::Memchr2<'a>),
}

impl Iterator for EitherMemchrIter<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Single(iter) => iter.next(),
            Self::Dual(iter) => iter.next(),
        }
    }
}

#[derive(Debug)]
pub(crate) enum SearchFilter {
    Empty,
    Substring(SubstringFilter),
    Regex(RegexFilter),
    InvalidRegex { message: String },
}

impl SearchFilter {
    pub(crate) fn compile(query: &str) -> Self {
        if query.is_empty() {
            return Self::Empty;
        }

        if let Some(pattern) = parse_regex_pattern(query) {
            return match RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
            {
                Ok(regex) => Self::Regex(RegexFilter { regex }),
                Err(error) => Self::InvalidRegex {
                    message: error.to_string(),
                },
            };
        }

        Self::Substring(SubstringFilter::new(query))
    }

    pub(crate) fn matches(&self, candidate: &str) -> bool {
        match self {
            Self::Empty => true,
            Self::Substring(filter) => filter.matches(candidate),
            Self::Regex(filter) => filter.matches(candidate),
            Self::InvalidRegex { .. } => false,
        }
    }

    pub(crate) const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    pub(crate) const fn error_message(&self) -> Option<&str> {
        match self {
            Self::InvalidRegex { message } => Some(message.as_str()),
            Self::Empty | Self::Substring(_) | Self::Regex(_) => None,
        }
    }
}

pub(crate) fn compile_cli_filter(
    query: Option<&str>,
    argument_name: &str,
) -> Result<Option<SearchFilter>> {
    let Some(query) = query else {
        return Ok(None);
    };

    let filter = SearchFilter::compile(query);
    if let Some(error) = filter.error_message() {
        bail!("invalid {argument_name} value {query:?}: {error}");
    }

    Ok(Some(filter))
}

fn parse_regex_pattern(query: &str) -> Option<&str> {
    if query.len() >= 2 && query.starts_with('/') && query.ends_with('/') {
        Some(&query[1..query.len() - 1])
    } else {
        None
    }
}
