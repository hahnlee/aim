//! Key character maps (`.kcm` files), as `KeyCharacterMap::load` parses
//! them at `android-16.0.0_r1` (frameworks/native/libs/input/
//! KeyCharacterMap.cpp, the keycode labels of `InputEventLabels.cpp`): what
//! an `InputDevice` carries for an app's `KeyCharacterMap`, and what
//! EventHub maps a keyboard's keys through (`mapKey`, `applyKeyBehavior`)
//! and the shell turns text into keys with (`getEvents`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;

use crate::keyboard::{
    self, AMETA_ALT_LEFT_ON, AMETA_ALT_ON, AMETA_ALT_RIGHT_ON, AMETA_CTRL_LEFT_ON, AMETA_CTRL_ON,
    AMETA_CTRL_RIGHT_ON, AMETA_META_LEFT_ON, AMETA_META_ON, AMETA_META_RIGHT_ON,
    AMETA_SHIFT_LEFT_ON, AMETA_SHIFT_ON, AMETA_SHIFT_RIGHT_ON,
};

/// `KeyCharacterMap::KeyboardType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardType {
    Unknown = 0,
    Numeric = 1,
    Predictive = 2,
    Alpha = 3,
    Full = 4,
    SpecialFunction = 5,
    Overlay = 6,
}

/// A key's behavior under a meta state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Behavior {
    pub meta_state: i32,
    pub character: u16,
    pub fallback_key_code: i32,
    pub replacement_key_code: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Key {
    pub label: u16,
    pub number: u16,
    /// Last declared first, as the parser pushes each to the front.
    pub behaviors: Vec<Behavior>,
}

/// A parsed key character map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyCharacterMap {
    pub load_file_name: String,
    pub kind: KeyboardType,
    pub layout_overlay_applied: bool,
    pub keys: BTreeMap<i32, Key>,
    pub keys_by_scan_code: BTreeMap<i32, i32>,
    pub keys_by_usage_code: BTreeMap<i32, i32>,
}

/// `AMETA_*` by the modifier labels a `.kcm` names them with.
const MODIFIERS: &[(&str, i32)] = &[
    ("shift", 0x01),
    ("lshift", 0x40),
    ("rshift", 0x80),
    ("alt", 0x02),
    ("lalt", 0x10),
    ("ralt", 0x20),
    ("ctrl", 0x1000),
    ("lctrl", 0x2000),
    ("rctrl", 0x4000),
    ("meta", 0x10000),
    ("lmeta", 0x20000),
    ("rmeta", 0x40000),
    ("sym", 0x04),
    ("fn", 0x08),
    ("capslock", 0x100000),
    ("numlock", 0x200000),
    ("scrolllock", 0x400000),
];

pub(crate) const WHITESPACE: &str = " \t\r";
const WHITESPACE_OR_PROPERTY_DELIMITER: &str = " \t\r,:";

/// `utils/Tokenizer`: a buffer read token by token within its lines.
pub(crate) struct Tokenizer<'a> {
    buffer: &'a [u8],
    at: usize,
    pub(crate) line: usize,
}

impl Tokenizer<'_> {
    pub(crate) fn new(buffer: &[u8]) -> Tokenizer<'_> {
        Tokenizer {
            buffer,
            at: 0,
            line: 1,
        }
    }
    pub(crate) fn is_eof(&self) -> bool {
        self.at >= self.buffer.len()
    }
    pub(crate) fn is_eol(&self) -> bool {
        self.is_eof() || self.buffer[self.at] == b'\n'
    }
    pub(crate) fn peek_char(&self) -> u8 {
        if self.is_eof() {
            0
        } else {
            self.buffer[self.at]
        }
    }
    pub(crate) fn next_char(&mut self) -> u8 {
        let ch = self.peek_char();
        if !self.is_eof() {
            self.at += 1;
        }
        ch
    }
    pub(crate) fn skip_delimiters(&mut self, delimiters: &str) {
        while !self.is_eol() && delimiters.as_bytes().contains(&self.buffer[self.at]) {
            self.at += 1;
        }
    }
    pub(crate) fn next_token(&mut self, delimiters: &str) -> String {
        let start = self.at;
        while !self.is_eol() && !delimiters.as_bytes().contains(&self.buffer[self.at]) {
            self.at += 1;
        }
        String::from_utf8_lossy(&self.buffer[start..self.at]).into_owned()
    }
    pub(crate) fn next_line(&mut self) {
        while !self.is_eof() && self.buffer[self.at] != b'\n' {
            self.at += 1;
        }
        if !self.is_eof() {
            self.at += 1;
        }
        self.line += 1;
    }
}

impl KeyCharacterMap {
    /// `KeyCharacterMap::load(filename, Format::BASE)` of `contents`.
    pub fn load(file_name: &str, contents: &[u8]) -> Result<KeyCharacterMap, String> {
        let mut map = KeyCharacterMap {
            load_file_name: file_name.to_string(),
            kind: KeyboardType::Unknown,
            layout_overlay_applied: false,
            keys: BTreeMap::new(),
            keys_by_scan_code: BTreeMap::new(),
            keys_by_usage_code: BTreeMap::new(),
        };
        let mut t = Tokenizer::new(contents);
        let mut key_code: Option<i32> = None;
        let err = |t: &Tokenizer<'_>, what: String| format!("{file_name}:{}: {what}", t.line);
        while !t.is_eof() {
            t.skip_delimiters(WHITESPACE);
            if !t.is_eol() && t.peek_char() != b'#' {
                match key_code {
                    None => {
                        let keyword = t.next_token(WHITESPACE);
                        t.skip_delimiters(WHITESPACE);
                        match keyword.as_str() {
                            "type" => map.parse_type(&mut t).map_err(|e| err(&t, e))?,
                            "map" => map.parse_map(&mut t).map_err(|e| err(&t, e))?,
                            "key" => {
                                key_code = Some(map.parse_key(&mut t).map_err(|e| err(&t, e))?)
                            }
                            other => {
                                return Err(err(&t, format!("Expected keyword, got '{other}'.")));
                            }
                        }
                    }
                    Some(code) => {
                        if map
                            .parse_key_property(&mut t, code)
                            .map_err(|e| err(&t, e))?
                        {
                            key_code = None;
                        }
                    }
                }
                t.skip_delimiters(WHITESPACE);
                if !t.is_eol() && t.peek_char() != b'#' {
                    return Err(err(&t, "Expected end of line or trailing comment.".into()));
                }
            }
            t.next_line();
        }
        if key_code.is_some() {
            return Err(err(
                &t,
                "Unterminated key description at end of file.".into(),
            ));
        }
        match map.kind {
            KeyboardType::Unknown => Err(err(
                &t,
                "Keyboard layout missing required keyboard 'type' declaration.".into(),
            )),
            KeyboardType::Overlay => Err(err(
                &t,
                "Base keyboard layout must specify a keyboard 'type' other than 'OVERLAY'.".into(),
            )),
            _ => Ok(map),
        }
    }

    fn parse_type(&mut self, t: &mut Tokenizer<'_>) -> Result<(), String> {
        if self.kind != KeyboardType::Unknown {
            return Err("Duplicate keyboard 'type' declaration.".into());
        }
        self.kind = match t.next_token(WHITESPACE).as_str() {
            "NUMERIC" => KeyboardType::Numeric,
            "PREDICTIVE" => KeyboardType::Predictive,
            "ALPHA" => KeyboardType::Alpha,
            "FULL" => KeyboardType::Full,
            "SPECIAL_FUNCTION" => KeyboardType::SpecialFunction,
            "OVERLAY" => KeyboardType::Overlay,
            other => return Err(format!("Expected keyboard type label, got '{other}'.")),
        };
        Ok(())
    }

    fn parse_map(&mut self, t: &mut Tokenizer<'_>) -> Result<(), String> {
        let keyword = t.next_token(WHITESPACE);
        if keyword != "key" {
            return Err(format!("Expected keyword after 'map', got '{keyword}'."));
        }
        t.skip_delimiters(WHITESPACE);
        let mut code_token = t.next_token(WHITESPACE);
        let usage = code_token == "usage";
        if usage {
            t.skip_delimiters(WHITESPACE);
            code_token = t.next_token(WHITESPACE);
        }
        let code = strtol(&code_token)
            .ok_or_else(|| format!("Expected key number, got '{code_token}'."))?;
        let map = if usage {
            &mut self.keys_by_usage_code
        } else {
            &mut self.keys_by_scan_code
        };
        if map.contains_key(&code) {
            return Err(format!("Duplicate entry for key '{code_token}'."));
        }
        t.skip_delimiters(WHITESPACE);
        let label = t.next_token(WHITESPACE);
        let key_code = key_code_by_label(&label)
            .ok_or_else(|| format!("Expected key code label, got '{label}'."))?;
        map.insert(code, key_code);
        Ok(())
    }

    fn parse_key(&mut self, t: &mut Tokenizer<'_>) -> Result<i32, String> {
        let label = t.next_token(WHITESPACE);
        let key_code = key_code_by_label(&label)
            .ok_or_else(|| format!("Expected key code label, got '{label}'."))?;
        if self.keys.contains_key(&key_code) {
            return Err(format!("Duplicate entry for key code '{label}'."));
        }
        t.skip_delimiters(WHITESPACE);
        let brace = t.next_token(WHITESPACE);
        if brace != "{" {
            return Err(format!(
                "Expected '{{' after key code label, got '{brace}'."
            ));
        }
        self.keys.insert(key_code, Key::default());
        Ok(key_code)
    }

    /// One property line of a key; `true` at its closing brace.
    fn parse_key_property(&mut self, t: &mut Tokenizer<'_>, key_code: i32) -> Result<bool, String> {
        let mut token = t.next_token(WHITESPACE_OR_PROPERTY_DELIMITER);
        if token == "}" {
            finish_key(self.keys.get_mut(&key_code).unwrap());
            return Ok(true);
        }
        enum Property {
            Label,
            Number,
            Meta(i32),
        }
        let mut properties = Vec::new();
        loop {
            properties.push(match token.as_str() {
                "label" => Property::Label,
                "number" => Property::Number,
                modifier => Property::Meta(parse_modifier(modifier).ok_or_else(|| {
                    format!("Expected a property name or modifier, got '{modifier}'.")
                })?),
            });
            t.skip_delimiters(WHITESPACE);
            if !t.is_eol() {
                match t.next_char() {
                    b':' => break,
                    b',' => {
                        t.skip_delimiters(WHITESPACE);
                        token = t.next_token(WHITESPACE_OR_PROPERTY_DELIMITER);
                        continue;
                    }
                    _ => {}
                }
            }
            return Err("Expected ',' or ':' after property name.".into());
        }
        t.skip_delimiters(WHITESPACE);
        let mut behavior = Behavior::default();
        let (mut have_character, mut have_fallback, mut have_replacement) = (false, false, false);
        loop {
            if t.peek_char() == b'\'' {
                let character = parse_character_literal(t)
                    .filter(|c| *c != 0)
                    .ok_or("Invalid character literal for key.")?;
                if have_character {
                    return Err("Cannot combine multiple character literals or 'none'.".into());
                }
                if have_replacement {
                    return Err("Cannot combine character literal with replace action.".into());
                }
                behavior.character = character;
                have_character = true;
            } else {
                token = t.next_token(WHITESPACE);
                match token.as_str() {
                    "none" => {
                        if have_character || have_replacement {
                            return Err(
                                "Cannot combine 'none' with a character or a replacement.".into()
                            );
                        }
                        have_character = true;
                    }
                    "fallback" | "replace" => {
                        let fallback = token == "fallback";
                        t.skip_delimiters(WHITESPACE);
                        let label = t.next_token(WHITESPACE);
                        let code = key_code_by_label(&label)
                            .ok_or_else(|| format!("Invalid key code label, got '{label}'."))?;
                        if !fallback && have_character {
                            return Err(
                                "Cannot combine character literal with replace action.".into()
                            );
                        }
                        if have_fallback || have_replacement {
                            return Err(
                                "Cannot combine multiple fallback/replacement key codes.".into()
                            );
                        }
                        if fallback {
                            behavior.fallback_key_code = code;
                            have_fallback = true;
                        } else {
                            behavior.replacement_key_code = code;
                            have_replacement = true;
                        }
                    }
                    _ => return Err("Expected a key behavior after ':'.".into()),
                }
            }
            t.skip_delimiters(WHITESPACE);
            if t.is_eol() || t.peek_char() == b'#' {
                break;
            }
        }
        let key = self.keys.get_mut(&key_code).unwrap();
        for property in properties {
            match property {
                Property::Label => {
                    if key.label != 0 {
                        return Err("Duplicate label for key.".into());
                    }
                    key.label = behavior.character;
                }
                Property::Number => {
                    if key.number != 0 {
                        return Err("Duplicate number for key.".into());
                    }
                    key.number = behavior.character;
                }
                Property::Meta(meta_state) => {
                    if key.behaviors.iter().any(|b| b.meta_state == meta_state) {
                        return Err("Duplicate key behavior for modifier.".into());
                    }
                    key.behaviors.insert(
                        0,
                        Behavior {
                            meta_state,
                            ..behavior
                        },
                    );
                }
            }
        }
        Ok(false)
    }

    /// `mapKey`: the key code a `map key` line gives the usage, else the
    /// scan code.
    pub fn map_key(&self, scan_code: i32, usage_code: i32) -> Option<i32> {
        if usage_code != 0
            && let Some(&code) = self.keys_by_usage_code.get(&usage_code)
        {
            return Some(code);
        }
        if scan_code != 0 {
            return self.keys_by_scan_code.get(&scan_code).copied();
        }
        None
    }

    /// `applyKeyRemapping`: the key codes the user remapped, of which the
    /// app host has none (`setKeyRemapping` comes from the input settings'
    /// key remapping, which it does not keep).
    pub fn apply_key_remapping(&self, key_code: i32) -> i32 {
        key_code
    }

    /// `findKeyCodesMappedToKeyCode`: none, without remappings.
    pub fn find_key_codes_mapped_to_key_code(&self, _key_code: i32) -> Vec<i32> {
        Vec::new()
    }

    /// `getKeyBehavior`: the first of the key's behaviors that matches.
    fn key_behavior(&self, key_code: i32, meta_state: i32) -> Option<&Behavior> {
        self.keys
            .get(&key_code)?
            .behaviors
            .iter()
            .find(|b| matches_meta_state(meta_state, b.meta_state))
    }

    /// `applyKeyBehavior`: a `replace` behavior's key code, with the
    /// modifiers it consumed taken off the meta state.
    pub fn apply_key_behavior(&self, key_code: i32, meta_state: i32) -> (i32, i32) {
        let Some(behavior) = self
            .key_behavior(key_code, meta_state)
            .filter(|b| b.replacement_key_code != 0)
        else {
            return (key_code, meta_state);
        };
        let consumed = behavior.meta_state;
        let mut to = meta_state & !consumed;
        for (either, sides) in [
            (AMETA_ALT_ON, AMETA_ALT_LEFT_ON | AMETA_ALT_RIGHT_ON),
            (AMETA_CTRL_ON, AMETA_CTRL_LEFT_ON | AMETA_CTRL_RIGHT_ON),
            (AMETA_SHIFT_ON, AMETA_SHIFT_LEFT_ON | AMETA_SHIFT_RIGHT_ON),
        ] {
            if consumed & either != 0 {
                to &= !sides;
            }
            if consumed & sides != 0 {
                to &= !either;
            }
        }
        (
            behavior.replacement_key_code,
            keyboard::normalize_meta_state(to),
        )
    }

    /// `findKey`: the first key (by key code) one of whose behaviors types
    /// `ch`, with the meta state of the first such behavior declared.
    fn find_key(&self, ch: u16) -> Option<(i32, i32)> {
        if ch == 0 {
            return None;
        }
        self.keys.iter().find_map(|(&code, key)| {
            key.behaviors
                .iter()
                .rfind(|b| b.character == ch)
                .map(|b| (code, b.meta_state))
        })
    }

    /// `getEvents`: each character's key, pressed with the modifiers it
    /// needs (`addMetaKeys`), as (key code, meta state, down); none if a
    /// character has no key.
    pub fn get_events(&self, chars: &[u16]) -> Option<Vec<(i32, i32, bool)>> {
        let mut events = Vec::new();
        for &ch in chars {
            let (key_code, meta_state) = self.find_key(ch)?;
            let mut current = 0;
            add_meta_keys(&mut events, meta_state, true, &mut current);
            events.push((key_code, current, true));
            events.push((key_code, current, false));
            add_meta_keys(&mut events, meta_state, false, &mut current);
        }
        Some(events)
    }
}

/// `matchesMetaState`: the behavior's modifiers all held, and no other
/// of Ctrl, Alt and Meta (either side matching a side or both).
fn matches_meta_state(event: i32, behavior: i32) -> bool {
    if event & behavior != behavior {
        return false;
    }
    const EXACT: i32 = AMETA_CTRL_ON
        | AMETA_CTRL_LEFT_ON
        | AMETA_CTRL_RIGHT_ON
        | AMETA_ALT_ON
        | AMETA_ALT_LEFT_ON
        | AMETA_ALT_RIGHT_ON
        | AMETA_META_ON
        | AMETA_META_LEFT_ON
        | AMETA_META_RIGHT_ON;
    let mut unmatched = event & !behavior & EXACT;
    for (either, sides) in [
        (AMETA_CTRL_ON, AMETA_CTRL_LEFT_ON | AMETA_CTRL_RIGHT_ON),
        (AMETA_ALT_ON, AMETA_ALT_LEFT_ON | AMETA_ALT_RIGHT_ON),
        (AMETA_META_ON, AMETA_META_LEFT_ON | AMETA_META_RIGHT_ON),
    ] {
        if behavior & either != 0 {
            unmatched &= !sides;
        } else if behavior & sides != 0 {
            unmatched &= !either;
        }
    }
    unmatched == 0
}

/// `addMetaKeys`: the locks, then Shift, Alt, Ctrl, Meta, Sym and Fn going
/// down, or the same in reverse going up.
fn add_meta_keys(events: &mut Vec<(i32, i32, bool)>, meta: i32, down: bool, current: &mut i32) {
    use keyboard::*;
    let locks = [
        (AKEYCODE_CAPS_LOCK, AMETA_CAPS_LOCK_ON),
        (AKEYCODE_NUM_LOCK, AMETA_NUM_LOCK_ON),
        (AKEYCODE_SCROLL_LOCK, AMETA_SCROLL_LOCK_ON),
    ];
    let doubles = [
        (
            AKEYCODE_SHIFT_LEFT,
            AMETA_SHIFT_LEFT_ON,
            AKEYCODE_SHIFT_RIGHT,
            AMETA_SHIFT_RIGHT_ON,
            AMETA_SHIFT_ON,
        ),
        (
            AKEYCODE_ALT_LEFT,
            AMETA_ALT_LEFT_ON,
            AKEYCODE_ALT_RIGHT,
            AMETA_ALT_RIGHT_ON,
            AMETA_ALT_ON,
        ),
        (
            AKEYCODE_CTRL_LEFT,
            AMETA_CTRL_LEFT_ON,
            AKEYCODE_CTRL_RIGHT,
            AMETA_CTRL_RIGHT_ON,
            AMETA_CTRL_ON,
        ),
        (
            AKEYCODE_META_LEFT,
            AMETA_META_LEFT_ON,
            AKEYCODE_META_RIGHT,
            AMETA_META_RIGHT_ON,
            AMETA_META_ON,
        ),
    ];
    let singles = [
        (AKEYCODE_SYM, AMETA_SYM_ON),
        (AKEYCODE_FUNCTION, AMETA_FUNCTION_ON),
    ];
    // `addSingleEphemeralMetaKey`.
    let single = |events: &mut Vec<(i32, i32, bool)>, current: &mut i32, code, mask| {
        if meta & mask == mask {
            *current = update_meta_state(code, down, *current);
            events.push((code, *current, down));
            true
        } else {
            false
        }
    };
    // `addDoubleEphemeralMetaKey`.
    let double =
        |events: &mut Vec<(i32, i32, bool)>,
         current: &mut i32,
         (left, left_mask, right, right_mask, either): (i32, i32, i32, i32, i32)| {
            let specific = single(events, current, left, left_mask)
                | single(events, current, right, right_mask);
            if !specific {
                single(events, current, left, either);
            }
        };
    // `addLockedMetaKey`.
    let lock = |events: &mut Vec<(i32, i32, bool)>, current: &mut i32, (code, mask): (i32, i32)| {
        if meta & mask == mask {
            *current = update_meta_state(code, true, *current);
            events.push((code, *current, true));
            *current = update_meta_state(code, false, *current);
            events.push((code, *current, false));
        }
    };
    if down {
        locks.into_iter().for_each(|l| lock(events, current, l));
        doubles.into_iter().for_each(|d| double(events, current, d));
        for (code, mask) in singles {
            single(events, current, code, mask);
        }
    } else {
        for (code, mask) in singles.into_iter().rev() {
            single(events, current, code, mask);
        }
        doubles
            .into_iter()
            .rev()
            .for_each(|d| double(events, current, d));
        locks
            .into_iter()
            .rev()
            .for_each(|l| lock(events, current, l));
    }
}

/// `Parser::finishKey`: a key without a number takes its last digit, else
/// its last symbol.
fn finish_key(key: &mut Key) {
    if key.number != 0 {
        return;
    }
    let (mut digit, mut symbol) = (0, 0);
    for b in &key.behaviors {
        let ch = b.character;
        if (u16::from(b'0')..=u16::from(b'9')).contains(&ch) {
            digit = ch;
        } else if b"()#*-+,.':;/".iter().any(|&s| u16::from(s) == ch) {
            symbol = ch;
        }
    }
    key.number = if digit != 0 { digit } else { symbol };
}

/// `Parser::parseModifier`: `base`, or modifiers joined by `+`, each once.
fn parse_modifier(token: &str) -> Option<i32> {
    if token == "base" {
        return Some(0);
    }
    let mut combined = 0;
    for part in token.split('+') {
        let meta = MODIFIERS.iter().find(|(label, _)| *label == part)?.1;
        if combined & meta != 0 {
            return None;
        }
        combined |= meta;
    }
    Some(combined)
}

/// `Parser::parseCharacterLiteral`: `'c'`, or an escape (`\n`, `\t`,
/// `\\`, `\'`, `\"`, `\uXXXX`), ending its token.
fn parse_character_literal(t: &mut Tokenizer<'_>) -> Option<u16> {
    if t.next_char() != b'\'' {
        return None;
    }
    let ch = t.next_char();
    let character = if ch == b'\\' {
        match t.next_char() {
            b'n' => u16::from(b'\n'),
            b't' => u16::from(b'\t'),
            b'\\' => u16::from(b'\\'),
            b'\'' => u16::from(b'\''),
            b'"' => u16::from(b'"'),
            b'u' => {
                let mut value = 0u16;
                for _ in 0..4 {
                    let digit = (t.next_char() as char).to_digit(16)?;
                    value = (value << 4) | digit as u16;
                }
                value
            }
            _ => return None,
        }
    } else if (32..=126).contains(&ch) && ch != b'\'' {
        u16::from(ch)
    } else {
        return None;
    };
    if t.next_char() != b'\'' || !t.next_token(WHITESPACE).is_empty() {
        return None;
    }
    Some(character)
}

/// `strtol(token, &end, 0)` of a whole token: decimal, `0x` hex or `0`
/// octal.
pub(crate) fn strtol(token: &str) -> Option<i32> {
    let (negative, digits) = match token.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, token.strip_prefix('+').unwrap_or(token)),
    };
    let value = if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        i64::from_str_radix(hex, 16).ok()?
    } else if digits.len() > 1 && digits.starts_with('0') {
        i64::from_str_radix(&digits[1..], 8).ok()?
    } else {
        digits.parse::<i64>().ok()?
    };
    Some((if negative { -value } else { value }) as i32)
}

/// `InputEventLookup::getKeyCodeByLabel`.
pub fn key_code_by_label(label: &str) -> Option<i32> {
    KEYCODES
        .iter()
        .find(|(name, _)| *name == label)
        .map(|(_, code)| *code)
}

/// `KEYCODES_SEQUENCE`: each `AKEYCODE_*`'s label and value.
const KEYCODES: &[(&str, i32)] = &[
    ("UNKNOWN", 0),
    ("SOFT_LEFT", 1),
    ("SOFT_RIGHT", 2),
    ("HOME", 3),
    ("BACK", 4),
    ("CALL", 5),
    ("ENDCALL", 6),
    ("0", 7),
    ("1", 8),
    ("2", 9),
    ("3", 10),
    ("4", 11),
    ("5", 12),
    ("6", 13),
    ("7", 14),
    ("8", 15),
    ("9", 16),
    ("STAR", 17),
    ("POUND", 18),
    ("DPAD_UP", 19),
    ("DPAD_DOWN", 20),
    ("DPAD_LEFT", 21),
    ("DPAD_RIGHT", 22),
    ("DPAD_CENTER", 23),
    ("VOLUME_UP", 24),
    ("VOLUME_DOWN", 25),
    ("POWER", 26),
    ("CAMERA", 27),
    ("CLEAR", 28),
    ("A", 29),
    ("B", 30),
    ("C", 31),
    ("D", 32),
    ("E", 33),
    ("F", 34),
    ("G", 35),
    ("H", 36),
    ("I", 37),
    ("J", 38),
    ("K", 39),
    ("L", 40),
    ("M", 41),
    ("N", 42),
    ("O", 43),
    ("P", 44),
    ("Q", 45),
    ("R", 46),
    ("S", 47),
    ("T", 48),
    ("U", 49),
    ("V", 50),
    ("W", 51),
    ("X", 52),
    ("Y", 53),
    ("Z", 54),
    ("COMMA", 55),
    ("PERIOD", 56),
    ("ALT_LEFT", 57),
    ("ALT_RIGHT", 58),
    ("SHIFT_LEFT", 59),
    ("SHIFT_RIGHT", 60),
    ("TAB", 61),
    ("SPACE", 62),
    ("SYM", 63),
    ("EXPLORER", 64),
    ("ENVELOPE", 65),
    ("ENTER", 66),
    ("DEL", 67),
    ("GRAVE", 68),
    ("MINUS", 69),
    ("EQUALS", 70),
    ("LEFT_BRACKET", 71),
    ("RIGHT_BRACKET", 72),
    ("BACKSLASH", 73),
    ("SEMICOLON", 74),
    ("APOSTROPHE", 75),
    ("SLASH", 76),
    ("AT", 77),
    ("NUM", 78),
    ("HEADSETHOOK", 79),
    ("FOCUS", 80),
    ("PLUS", 81),
    ("MENU", 82),
    ("NOTIFICATION", 83),
    ("SEARCH", 84),
    ("MEDIA_PLAY_PAUSE", 85),
    ("MEDIA_STOP", 86),
    ("MEDIA_NEXT", 87),
    ("MEDIA_PREVIOUS", 88),
    ("MEDIA_REWIND", 89),
    ("MEDIA_FAST_FORWARD", 90),
    ("MUTE", 91),
    ("PAGE_UP", 92),
    ("PAGE_DOWN", 93),
    ("PICTSYMBOLS", 94),
    ("SWITCH_CHARSET", 95),
    ("BUTTON_A", 96),
    ("BUTTON_B", 97),
    ("BUTTON_C", 98),
    ("BUTTON_X", 99),
    ("BUTTON_Y", 100),
    ("BUTTON_Z", 101),
    ("BUTTON_L1", 102),
    ("BUTTON_R1", 103),
    ("BUTTON_L2", 104),
    ("BUTTON_R2", 105),
    ("BUTTON_THUMBL", 106),
    ("BUTTON_THUMBR", 107),
    ("BUTTON_START", 108),
    ("BUTTON_SELECT", 109),
    ("BUTTON_MODE", 110),
    ("ESCAPE", 111),
    ("FORWARD_DEL", 112),
    ("CTRL_LEFT", 113),
    ("CTRL_RIGHT", 114),
    ("CAPS_LOCK", 115),
    ("SCROLL_LOCK", 116),
    ("META_LEFT", 117),
    ("META_RIGHT", 118),
    ("FUNCTION", 119),
    ("SYSRQ", 120),
    ("BREAK", 121),
    ("MOVE_HOME", 122),
    ("MOVE_END", 123),
    ("INSERT", 124),
    ("FORWARD", 125),
    ("MEDIA_PLAY", 126),
    ("MEDIA_PAUSE", 127),
    ("MEDIA_CLOSE", 128),
    ("MEDIA_EJECT", 129),
    ("MEDIA_RECORD", 130),
    ("F1", 131),
    ("F2", 132),
    ("F3", 133),
    ("F4", 134),
    ("F5", 135),
    ("F6", 136),
    ("F7", 137),
    ("F8", 138),
    ("F9", 139),
    ("F10", 140),
    ("F11", 141),
    ("F12", 142),
    ("NUM_LOCK", 143),
    ("NUMPAD_0", 144),
    ("NUMPAD_1", 145),
    ("NUMPAD_2", 146),
    ("NUMPAD_3", 147),
    ("NUMPAD_4", 148),
    ("NUMPAD_5", 149),
    ("NUMPAD_6", 150),
    ("NUMPAD_7", 151),
    ("NUMPAD_8", 152),
    ("NUMPAD_9", 153),
    ("NUMPAD_DIVIDE", 154),
    ("NUMPAD_MULTIPLY", 155),
    ("NUMPAD_SUBTRACT", 156),
    ("NUMPAD_ADD", 157),
    ("NUMPAD_DOT", 158),
    ("NUMPAD_COMMA", 159),
    ("NUMPAD_ENTER", 160),
    ("NUMPAD_EQUALS", 161),
    ("NUMPAD_LEFT_PAREN", 162),
    ("NUMPAD_RIGHT_PAREN", 163),
    ("VOLUME_MUTE", 164),
    ("INFO", 165),
    ("CHANNEL_UP", 166),
    ("CHANNEL_DOWN", 167),
    ("ZOOM_IN", 168),
    ("ZOOM_OUT", 169),
    ("TV", 170),
    ("WINDOW", 171),
    ("GUIDE", 172),
    ("DVR", 173),
    ("BOOKMARK", 174),
    ("CAPTIONS", 175),
    ("SETTINGS", 176),
    ("TV_POWER", 177),
    ("TV_INPUT", 178),
    ("STB_POWER", 179),
    ("STB_INPUT", 180),
    ("AVR_POWER", 181),
    ("AVR_INPUT", 182),
    ("PROG_RED", 183),
    ("PROG_GREEN", 184),
    ("PROG_YELLOW", 185),
    ("PROG_BLUE", 186),
    ("APP_SWITCH", 187),
    ("BUTTON_1", 188),
    ("BUTTON_2", 189),
    ("BUTTON_3", 190),
    ("BUTTON_4", 191),
    ("BUTTON_5", 192),
    ("BUTTON_6", 193),
    ("BUTTON_7", 194),
    ("BUTTON_8", 195),
    ("BUTTON_9", 196),
    ("BUTTON_10", 197),
    ("BUTTON_11", 198),
    ("BUTTON_12", 199),
    ("BUTTON_13", 200),
    ("BUTTON_14", 201),
    ("BUTTON_15", 202),
    ("BUTTON_16", 203),
    ("LANGUAGE_SWITCH", 204),
    ("MANNER_MODE", 205),
    ("3D_MODE", 206),
    ("CONTACTS", 207),
    ("CALENDAR", 208),
    ("MUSIC", 209),
    ("CALCULATOR", 210),
    ("ZENKAKU_HANKAKU", 211),
    ("EISU", 212),
    ("MUHENKAN", 213),
    ("HENKAN", 214),
    ("KATAKANA_HIRAGANA", 215),
    ("YEN", 216),
    ("RO", 217),
    ("KANA", 218),
    ("ASSIST", 219),
    ("BRIGHTNESS_DOWN", 220),
    ("BRIGHTNESS_UP", 221),
    ("MEDIA_AUDIO_TRACK", 222),
    ("SLEEP", 223),
    ("WAKEUP", 224),
    ("PAIRING", 225),
    ("MEDIA_TOP_MENU", 226),
    ("11", 227),
    ("12", 228),
    ("LAST_CHANNEL", 229),
    ("TV_DATA_SERVICE", 230),
    ("VOICE_ASSIST", 231),
    ("TV_RADIO_SERVICE", 232),
    ("TV_TELETEXT", 233),
    ("TV_NUMBER_ENTRY", 234),
    ("TV_TERRESTRIAL_ANALOG", 235),
    ("TV_TERRESTRIAL_DIGITAL", 236),
    ("TV_SATELLITE", 237),
    ("TV_SATELLITE_BS", 238),
    ("TV_SATELLITE_CS", 239),
    ("TV_SATELLITE_SERVICE", 240),
    ("TV_NETWORK", 241),
    ("TV_ANTENNA_CABLE", 242),
    ("TV_INPUT_HDMI_1", 243),
    ("TV_INPUT_HDMI_2", 244),
    ("TV_INPUT_HDMI_3", 245),
    ("TV_INPUT_HDMI_4", 246),
    ("TV_INPUT_COMPOSITE_1", 247),
    ("TV_INPUT_COMPOSITE_2", 248),
    ("TV_INPUT_COMPONENT_1", 249),
    ("TV_INPUT_COMPONENT_2", 250),
    ("TV_INPUT_VGA_1", 251),
    ("TV_AUDIO_DESCRIPTION", 252),
    ("TV_AUDIO_DESCRIPTION_MIX_UP", 253),
    ("TV_AUDIO_DESCRIPTION_MIX_DOWN", 254),
    ("TV_ZOOM_MODE", 255),
    ("TV_CONTENTS_MENU", 256),
    ("TV_MEDIA_CONTEXT_MENU", 257),
    ("TV_TIMER_PROGRAMMING", 258),
    ("HELP", 259),
    ("NAVIGATE_PREVIOUS", 260),
    ("NAVIGATE_NEXT", 261),
    ("NAVIGATE_IN", 262),
    ("NAVIGATE_OUT", 263),
    ("STEM_PRIMARY", 264),
    ("STEM_1", 265),
    ("STEM_2", 266),
    ("STEM_3", 267),
    ("DPAD_UP_LEFT", 268),
    ("DPAD_DOWN_LEFT", 269),
    ("DPAD_UP_RIGHT", 270),
    ("DPAD_DOWN_RIGHT", 271),
    ("MEDIA_SKIP_FORWARD", 272),
    ("MEDIA_SKIP_BACKWARD", 273),
    ("MEDIA_STEP_FORWARD", 274),
    ("MEDIA_STEP_BACKWARD", 275),
    ("SOFT_SLEEP", 276),
    ("CUT", 277),
    ("COPY", 278),
    ("PASTE", 279),
    ("SYSTEM_NAVIGATION_UP", 280),
    ("SYSTEM_NAVIGATION_DOWN", 281),
    ("SYSTEM_NAVIGATION_LEFT", 282),
    ("SYSTEM_NAVIGATION_RIGHT", 283),
    ("ALL_APPS", 284),
    ("REFRESH", 285),
    ("THUMBS_UP", 286),
    ("THUMBS_DOWN", 287),
    ("PROFILE_SWITCH", 288),
    ("VIDEO_APP_1", 289),
    ("VIDEO_APP_2", 290),
    ("VIDEO_APP_3", 291),
    ("VIDEO_APP_4", 292),
    ("VIDEO_APP_5", 293),
    ("VIDEO_APP_6", 294),
    ("VIDEO_APP_7", 295),
    ("VIDEO_APP_8", 296),
    ("FEATURED_APP_1", 297),
    ("FEATURED_APP_2", 298),
    ("FEATURED_APP_3", 299),
    ("FEATURED_APP_4", 300),
    ("DEMO_APP_1", 301),
    ("DEMO_APP_2", 302),
    ("DEMO_APP_3", 303),
    ("DEMO_APP_4", 304),
    ("KEYBOARD_BACKLIGHT_DOWN", 305),
    ("KEYBOARD_BACKLIGHT_UP", 306),
    ("KEYBOARD_BACKLIGHT_TOGGLE", 307),
    ("STYLUS_BUTTON_PRIMARY", 308),
    ("STYLUS_BUTTON_SECONDARY", 309),
    ("STYLUS_BUTTON_TERTIARY", 310),
    ("STYLUS_BUTTON_TAIL", 311),
    ("RECENT_APPS", 312),
    ("MACRO_1", 313),
    ("MACRO_2", 314),
    ("MACRO_3", 315),
    ("MACRO_4", 316),
    ("EMOJI_PICKER", 317),
    ("SCREENSHOT", 318),
    ("DICTATE", 319),
    ("NEW", 320),
    ("CLOSE", 321),
    ("DO_NOT_DISTURB", 322),
    ("PRINT", 323),
    ("LOCK", 324),
    ("FULLSCREEN", 325),
    ("F13", 326),
    ("F14", 327),
    ("F15", 328),
    ("F16", 329),
    ("F17", 330),
    ("F18", 331),
    ("F19", 332),
    ("F20", 333),
    ("F21", 334),
    ("F22", 335),
    ("F23", 336),
    ("F24", 337),
];

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# A sample, in Virtual.kcm's form.\ntype FULL\n\nkey A {\n    label:                              'A'\n    base:                               'a'\n    shift, capslock:                    'A'\n    shift+capslock:                     'a'\n}\n\nkey 1 {\n    label:  '1'\n    base:   '1'\n    shift:  '!'\n}\n\nkey SPACE {\n    label: ' '\n    base: ' '\n    alt, meta: fallback SEARCH\n    ctrl: fallback LANGUAGE_SWITCH\n}\n\nkey C {\n    alt: '\\u00e7'\n}\n\nmap key 0x31 SPACE\n";

    #[test]
    fn parses_keys_as_the_original() {
        let map = KeyCharacterMap::load("/x.kcm", SAMPLE.as_bytes()).unwrap();
        assert_eq!(map.kind, KeyboardType::Full);
        let a = &map.keys[&29];
        assert_eq!((a.label, a.number), (u16::from(b'A'), 0));
        // Behaviors last declared first; one per modifier of a line.
        let metas: Vec<i32> = a.behaviors.iter().map(|b| b.meta_state).collect();
        assert_eq!(metas, [0x100001, 0x100000, 0x01, 0]);
        // A key's number defaults to its digit.
        assert_eq!(map.keys[&8].number, u16::from(b'1'));
        let space = &map.keys[&62];
        assert_eq!(
            space.behaviors[0],
            Behavior {
                meta_state: 0x1000,
                fallback_key_code: 204,
                ..Default::default()
            }
        );
        assert_eq!(map.keys[&31].behaviors[0].character, 0xe7);
        assert_eq!(map.keys_by_scan_code[&0x31], 62);
        assert_eq!(map.map_key(0x31, 0), Some(62));
    }

    /// `getEvents`: a capital takes Shift around it.
    #[test]
    fn text_becomes_keys_with_their_modifiers() {
        let map = KeyCharacterMap::load("/x.kcm", SAMPLE.as_bytes()).unwrap();
        let events = map.get_events(&[u16::from(b'A'), u16::from(b'!')]).unwrap();
        assert_eq!(
            events,
            [
                (59, 0x41, true),
                (29, 0x41, true),
                (29, 0x41, false),
                (59, 0, false),
                (59, 0x41, true),
                (8, 0x41, true),
                (8, 0x41, false),
                (59, 0, false),
            ]
        );
        assert!(map.get_events(&[u16::from(b'z')]).is_none());
    }

    #[test]
    fn refuses_malformed_maps() {
        assert!(KeyCharacterMap::load("/x.kcm", b"key A {\n}\n").is_err());
        assert!(KeyCharacterMap::load("/x.kcm", b"type FULL\nkey A {\n base: 'a\n}\n").is_err());
        assert!(KeyCharacterMap::load("/x.kcm", b"type OVERLAY\n").is_err());
        assert_eq!(strtol("0x10"), Some(16));
        assert_eq!(strtol("010"), Some(8));
        assert_eq!(key_code_by_label("F24"), Some(337));
    }
}
