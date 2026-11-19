// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: Pure-Rust linear-time Thompson NFA regex compiler and line matcher

use std::collections::BTreeSet;

/// Compiled linear-time Thompson NFA regular expression.
#[derive(Debug, Clone)]
pub struct Regex {
    states: Vec<State>,
    start: usize,
    case_insensitive: bool,
}

#[derive(Debug, Clone)]
enum State {
    Split(usize, usize),
    Match,
    Atom { kind: AtomKind, next: usize },
    Anchor { kind: AnchorKind, next: usize },
}

#[derive(Debug, Clone)]
enum AtomKind {
    Any,
    Literal(char),
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
        shorthands: Vec<Shorthand>,
    },
    Shorthand(Shorthand),
}

#[derive(Debug, Clone, Copy)]
enum Shorthand {
    Digit,
    NonDigit,
    Word,
    NonWord,
    Space,
    NonSpace,
}

#[derive(Debug, Clone, Copy)]
enum AnchorKind {
    Start,
    End,
    WordBoundary,
    NonWordBoundary,
}

struct Frag {
    start: usize,
    outs: Vec<OutPatch>,
}

#[derive(Debug, Clone, Copy)]
enum OutPatch {
    StateNext(usize),
    SplitFirst(usize),
    SplitSecond(usize),
}

impl Regex {
    /// Compiles `pattern` into a linear-time Thompson NFA regex.
    pub fn compile(pattern: &str, mut case_insensitive: bool) -> Result<Self, String> {
        let mut pat = pattern;
        if let Some(rest) = pat.strip_prefix("(?i)") {
            case_insensitive = true;
            pat = rest;
        }
        let chars: Vec<char> = pat.chars().collect();
        let mut parser = Parser {
            chars: &chars,
            pos: 0,
            states: Vec::new(),
            case_insensitive,
        };
        let frag = parser.parse_alt()?;
        if parser.pos < parser.chars.len() {
            return Err(format!(
                "unexpected trailing character '{}' at position {}",
                parser.chars[parser.pos], parser.pos
            ));
        }
        let match_id = parser.push(State::Match);
        parser.patch(&frag.outs, match_id);
        Ok(Self {
            states: parser.states,
            start: frag.start,
            case_insensitive,
        })
    }

    /// Returns `true` if any substring of `line` matches this compiled regex.
    #[must_use]
    pub fn is_match(&self, line: &str) -> bool {
        let chars: Vec<char> = line.chars().collect();
        for start_pos in 0..=chars.len() {
            if self.matches_at(&chars, start_pos) {
                return true;
            }
        }
        false
    }

    fn matches_at(&self, chars: &[char], start_pos: usize) -> bool {
        let mut cur = BTreeSet::new();
        self.add_epsilon_closure(self.start, chars, start_pos, &mut cur);
        if cur
            .iter()
            .any(|&id| matches!(self.states[id], State::Match))
        {
            return true;
        }
        for pos in start_pos..chars.len() {
            if cur.is_empty() {
                return false;
            }
            let ch = chars[pos];
            let mut next = BTreeSet::new();
            for &id in &cur {
                if let State::Atom { kind, next: target } = &self.states[id]
                    && kind.matches(ch, self.case_insensitive)
                {
                    self.add_epsilon_closure(*target, chars, pos + 1, &mut next);
                }
            }
            cur = next;
            if cur
                .iter()
                .any(|&id| matches!(self.states[id], State::Match))
            {
                return true;
            }
        }
        false
    }

    fn add_epsilon_closure(
        &self,
        start_id: usize,
        chars: &[char],
        pos: usize,
        set: &mut BTreeSet<usize>,
    ) {
        let mut stack = vec![start_id];
        while let Some(id) = stack.pop() {
            if !set.insert(id) {
                continue;
            }
            match &self.states[id] {
                State::Split(a, b) => {
                    stack.push(*a);
                    stack.push(*b);
                }
                State::Anchor { kind, next } => {
                    if kind.matches(chars, pos) {
                        stack.push(*next);
                    }
                }
                State::Match | State::Atom { .. } => {}
            }
        }
    }
}

impl AtomKind {
    fn matches(&self, ch: char, case_insensitive: bool) -> bool {
        match self {
            Self::Any => ch != '\n',
            Self::Literal(lit) => chars_equal(ch, *lit, case_insensitive),
            Self::Shorthand(sh) => sh.matches(ch),
            Self::Class {
                negated,
                ranges,
                shorthands,
            } => {
                let in_sh = shorthands.iter().any(|s| s.matches(ch));
                let in_range = ranges.iter().any(|&(a, b)| {
                    if case_insensitive {
                        let lc = ch.to_ascii_lowercase();
                        let uc = ch.to_ascii_uppercase();
                        (a.to_ascii_lowercase() <= lc && lc <= b.to_ascii_lowercase())
                            || (a.to_ascii_uppercase() <= uc && uc <= b.to_ascii_uppercase())
                    } else {
                        a <= ch && ch <= b
                    }
                });
                let matched = in_sh || in_range;
                if *negated { !matched } else { matched }
            }
        }
    }
}

impl Shorthand {
    fn matches(self, ch: char) -> bool {
        match self {
            Self::Digit => ch.is_ascii_digit(),
            Self::NonDigit => !ch.is_ascii_digit(),
            Self::Word => ch.is_ascii_alphanumeric() || ch == '_',
            Self::NonWord => !(ch.is_ascii_alphanumeric() || ch == '_'),
            Self::Space => ch.is_whitespace(),
            Self::NonSpace => !ch.is_whitespace(),
        }
    }
}

impl AnchorKind {
    fn matches(self, chars: &[char], pos: usize) -> bool {
        match self {
            Self::Start => pos == 0,
            Self::End => pos == chars.len(),
            Self::WordBoundary => is_word_boundary(chars, pos),
            Self::NonWordBoundary => !is_word_boundary(chars, pos),
        }
    }
}

fn is_word_boundary(chars: &[char], pos: usize) -> bool {
    let left = pos > 0 && (chars[pos - 1].is_ascii_alphanumeric() || chars[pos - 1] == '_');
    let right = pos < chars.len() && (chars[pos].is_ascii_alphanumeric() || chars[pos] == '_');
    left != right
}

fn chars_equal(a: char, b: char, case_insensitive: bool) -> bool {
    if case_insensitive {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
    states: Vec<State>,
    case_insensitive: bool,
}

impl<'a> Parser<'a> {
    fn push(&mut self, state: State) -> usize {
        let id = self.states.len();
        self.states.push(state);
        id
    }

    fn patch(&mut self, outs: &[OutPatch], target: usize) {
        for &patch in outs {
            match patch {
                OutPatch::StateNext(id) => match &mut self.states[id] {
                    State::Atom { next, .. } | State::Anchor { next, .. } => *next = target,
                    State::Split(_, _) | State::Match => {}
                },
                OutPatch::SplitFirst(id) => {
                    if let State::Split(first, _) = &mut self.states[id] {
                        *first = target;
                    }
                }
                OutPatch::SplitSecond(id) => {
                    if let State::Split(_, second) = &mut self.states[id] {
                        *second = target;
                    }
                }
            }
        }
    }

    fn empty_frag(&mut self) -> Frag {
        let id = self.push(State::Split(0, 0));
        Frag {
            start: id,
            outs: vec![OutPatch::SplitFirst(id), OutPatch::SplitSecond(id)],
        }
    }

    fn parse_alt(&mut self) -> Result<Frag, String> {
        let mut left = self.parse_concat()?;
        while self.pos < self.chars.len() && self.chars[self.pos] == '|' {
            self.pos += 1;
            let right = self.parse_concat()?;
            let split = self.push(State::Split(left.start, right.start));
            let mut outs = left.outs;
            outs.extend(right.outs);
            left = Frag { start: split, outs };
        }
        Ok(left)
    }

    fn parse_concat(&mut self) -> Result<Frag, String> {
        let mut items = Vec::new();
        while self.pos < self.chars.len() {
            let ch = self.chars[self.pos];
            if ch == '|' || ch == ')' {
                break;
            }
            items.push(self.parse_quantified()?);
        }
        if items.is_empty() {
            return Ok(self.empty_frag());
        }
        let mut iter = items.into_iter();
        let mut acc = iter
            .next()
            .ok_or_else(|| "empty concatenation".to_owned())?;
        for next in iter {
            self.patch(&acc.outs, next.start);
            acc.outs = next.outs;
        }
        Ok(acc)
    }

    fn parse_quantified(&mut self) -> Result<Frag, String> {
        let atom_start_pos = self.pos;
        let frag = self.parse_atom()?;
        if self.pos >= self.chars.len() {
            return Ok(frag);
        }
        let q = self.chars[self.pos];
        let result = match q {
            '*' => {
                self.pos += 1;
                let split = self.push(State::Split(frag.start, 0));
                self.patch(&frag.outs, split);
                Frag {
                    start: split,
                    outs: vec![OutPatch::SplitSecond(split)],
                }
            }
            '+' => {
                self.pos += 1;
                let split = self.push(State::Split(frag.start, 0));
                self.patch(&frag.outs, split);
                Frag {
                    start: frag.start,
                    outs: vec![OutPatch::SplitSecond(split)],
                }
            }
            '?' => {
                self.pos += 1;
                let split = self.push(State::Split(frag.start, 0));
                let mut outs = frag.outs;
                outs.push(OutPatch::SplitSecond(split));
                Frag { start: split, outs }
            }
            '{' => {
                if let Some((min, max)) = self.try_parse_brace_quantifier()? {
                    let atom_end_pos = self.pos;
                    let sub_slice = &self.chars[atom_start_pos..atom_end_pos];
                    // Re-parse bounded repetition cleanly by cloning states from `sub_slice` without quantifier.
                    let raw_atom_len = sub_slice
                        .iter()
                        .rposition(|&c| c == '{')
                        .unwrap_or(sub_slice.len());
                    let atom_chars = &sub_slice[..raw_atom_len];
                    return self.build_bounded_repeat(frag, atom_chars, min, max);
                }
                frag
            }
            _ => frag,
        };
        if self.pos < self.chars.len()
            && self.chars[self.pos] == '?'
            && matches!(q, '*' | '+' | '?')
        {
            self.pos += 1;
        }
        Ok(result)
    }

    fn build_bounded_repeat(
        &mut self,
        first: Frag,
        atom_chars: &[char],
        min: usize,
        max: Option<usize>,
    ) -> Result<Frag, String> {
        let mut parts: Vec<Frag> = Vec::new();
        let mut used_first = Some(first);
        let mut spawn_atom = |this: &mut Self| -> Result<Frag, String> {
            if let Some(f) = used_first.take() {
                return Ok(f);
            }
            let mut sub = Parser {
                chars: atom_chars,
                pos: 0,
                states: std::mem::take(&mut this.states),
                case_insensitive: this.case_insensitive,
            };
            let frag = sub.parse_atom()?;
            this.states = sub.states;
            Ok(frag)
        };

        for _ in 0..min {
            parts.push(spawn_atom(self)?);
        }
        if let Some(max_val) = max {
            if max_val < min {
                return Err(format!("invalid quantifier {{{min},{max_val}}}"));
            }
            for _ in min..max_val {
                let f = spawn_atom(self)?;
                let split = self.push(State::Split(f.start, 0));
                let mut outs = f.outs;
                outs.push(OutPatch::SplitSecond(split));
                parts.push(Frag { start: split, outs });
            }
        } else {
            let f = spawn_atom(self)?;
            let split = self.push(State::Split(f.start, 0));
            self.patch(&f.outs, split);
            parts.push(Frag {
                start: split,
                outs: vec![OutPatch::SplitSecond(split)],
            });
        }
        if self.pos < self.chars.len() && self.chars[self.pos] == '?' {
            self.pos += 1;
        }
        if parts.is_empty() {
            return Ok(self.empty_frag());
        }
        let mut iter = parts.into_iter();
        let mut acc = iter
            .next()
            .ok_or_else(|| "empty bounded repeat".to_owned())?;
        for next in iter {
            self.patch(&acc.outs, next.start);
            acc.outs = next.outs;
        }
        Ok(acc)
    }

    fn try_parse_brace_quantifier(&mut self) -> Result<Option<(usize, Option<usize>)>, String> {
        let saved = self.pos;
        self.pos += 1;
        let Some(min) = self.read_uint() else {
            self.pos = saved;
            return Ok(None);
        };
        if self.pos >= self.chars.len() {
            self.pos = saved;
            return Ok(None);
        }
        if self.chars[self.pos] == '}' {
            self.pos += 1;
            return Ok(Some((min.min(64), Some(min.min(64)))));
        }
        if self.chars[self.pos] != ',' {
            self.pos = saved;
            return Ok(None);
        }
        self.pos += 1;
        if self.pos < self.chars.len() && self.chars[self.pos] == '}' {
            self.pos += 1;
            return Ok(Some((min.min(64), None)));
        }
        let Some(max) = self.read_uint() else {
            self.pos = saved;
            return Ok(None);
        };
        if self.pos < self.chars.len() && self.chars[self.pos] == '}' {
            self.pos += 1;
            return Ok(Some((min.min(64), Some(max.min(64)))));
        }
        self.pos = saved;
        Ok(None)
    }

    fn read_uint(&mut self) -> Option<usize> {
        let start = self.pos;
        while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos == start {
            return None;
        }
        let s: String = self.chars[start..self.pos].iter().collect();
        s.parse::<usize>().ok()
    }

    fn parse_atom(&mut self) -> Result<Frag, String> {
        if self.pos >= self.chars.len() {
            return Err("unexpected end of regex".to_owned());
        }
        let ch = self.chars[self.pos];
        match ch {
            '(' => {
                self.pos += 1;
                if self.pos + 1 < self.chars.len()
                    && self.chars[self.pos] == '?'
                    && self.chars[self.pos + 1] == ':'
                {
                    self.pos += 2;
                }
                let inner = self.parse_alt()?;
                if self.pos >= self.chars.len() || self.chars[self.pos] != ')' {
                    return Err("unclosed '(' in regex".to_owned());
                }
                self.pos += 1;
                Ok(inner)
            }
            '[' => self.parse_class(),
            '.' => {
                self.pos += 1;
                let id = self.push(State::Atom {
                    kind: AtomKind::Any,
                    next: 0,
                });
                Ok(Frag {
                    start: id,
                    outs: vec![OutPatch::StateNext(id)],
                })
            }
            '^' => {
                self.pos += 1;
                let id = self.push(State::Anchor {
                    kind: AnchorKind::Start,
                    next: 0,
                });
                Ok(Frag {
                    start: id,
                    outs: vec![OutPatch::StateNext(id)],
                })
            }
            '$' => {
                self.pos += 1;
                let id = self.push(State::Anchor {
                    kind: AnchorKind::End,
                    next: 0,
                });
                Ok(Frag {
                    start: id,
                    outs: vec![OutPatch::StateNext(id)],
                })
            }
            '\\' => self.parse_escape(),
            '*' | '+' | '?' => Err(format!(
                "unexpected quantifier '{ch}' at position {}",
                self.pos
            )),
            _ => {
                self.pos += 1;
                let id = self.push(State::Atom {
                    kind: AtomKind::Literal(ch),
                    next: 0,
                });
                Ok(Frag {
                    start: id,
                    outs: vec![OutPatch::StateNext(id)],
                })
            }
        }
    }

    fn parse_escape(&mut self) -> Result<Frag, String> {
        self.pos += 1;
        if self.pos >= self.chars.len() {
            return Err("trailing '\\' in regex".to_owned());
        }
        let esc = self.chars[self.pos];
        self.pos += 1;
        let id = match esc {
            'b' => self.push(State::Anchor {
                kind: AnchorKind::WordBoundary,
                next: 0,
            }),
            'B' => self.push(State::Anchor {
                kind: AnchorKind::NonWordBoundary,
                next: 0,
            }),
            'd' => self.push_shorthand(Shorthand::Digit),
            'D' => self.push_shorthand(Shorthand::NonDigit),
            'w' => self.push_shorthand(Shorthand::Word),
            'W' => self.push_shorthand(Shorthand::NonWord),
            's' => self.push_shorthand(Shorthand::Space),
            'S' => self.push_shorthand(Shorthand::NonSpace),
            'n' => self.push_literal('\n'),
            'r' => self.push_literal('\r'),
            't' => self.push_literal('\t'),
            other => self.push_literal(other),
        };
        Ok(Frag {
            start: id,
            outs: vec![OutPatch::StateNext(id)],
        })
    }

    fn push_shorthand(&mut self, sh: Shorthand) -> usize {
        self.push(State::Atom {
            kind: AtomKind::Shorthand(sh),
            next: 0,
        })
    }

    fn push_literal(&mut self, ch: char) -> usize {
        self.push(State::Atom {
            kind: AtomKind::Literal(ch),
            next: 0,
        })
    }

    fn parse_class(&mut self) -> Result<Frag, String> {
        self.pos += 1;
        let negated = self.pos < self.chars.len() && self.chars[self.pos] == '^';
        if negated {
            self.pos += 1;
        }
        let mut ranges = Vec::new();
        let mut shorthands = Vec::new();
        let mut first = true;

        while self.pos < self.chars.len() {
            if self.chars[self.pos] == ']' && !first {
                self.pos += 1;
                let id = self.push(State::Atom {
                    kind: AtomKind::Class {
                        negated,
                        ranges,
                        shorthands,
                    },
                    next: 0,
                });
                return Ok(Frag {
                    start: id,
                    outs: vec![OutPatch::StateNext(id)],
                });
            }
            first = false;
            if self.chars[self.pos] == '\\' && self.pos + 1 < self.chars.len() {
                let esc = self.chars[self.pos + 1];
                self.pos += 2;
                match esc {
                    'd' => shorthands.push(Shorthand::Digit),
                    'D' => shorthands.push(Shorthand::NonDigit),
                    'w' => shorthands.push(Shorthand::Word),
                    'W' => shorthands.push(Shorthand::NonWord),
                    's' => shorthands.push(Shorthand::Space),
                    'S' => shorthands.push(Shorthand::NonSpace),
                    'n' => ranges.push(('\n', '\n')),
                    'r' => ranges.push(('\r', '\r')),
                    't' => ranges.push(('\t', '\t')),
                    other => ranges.push((other, other)),
                }
                continue;
            }
            let start = self.chars[self.pos];
            self.pos += 1;
            if self.pos + 1 < self.chars.len()
                && self.chars[self.pos] == '-'
                && self.chars[self.pos + 1] != ']'
            {
                let end = self.chars[self.pos + 1];
                self.pos += 2;
                ranges.push((start, end));
            } else {
                ranges.push((start, start));
            }
        }
        Err("unclosed '[' in regex".to_owned())
    }
}
