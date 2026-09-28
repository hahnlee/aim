//! Port of system/core/init/tokenizer.cpp (`next_token`).
//!
//! The byte-level behavior is kept exactly: `#` starts a comment only at a
//! token start, quotes may appear mid-token (`a"b c"d` is one token), a
//! backslash escapes `\n \r \t \\` and any other byte is copied, a backslash
//! before a newline continues the line, and an unterminated quote ends the
//! file without emitting the partial line.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Token {
    Eof,
    Text(String),
    Newline,
}

pub(crate) struct Tokenizer {
    data: Vec<u8>,
    ptr: usize,
    /// `parse_state.line`: incremented by line continuations here and by the
    /// parser on every `T_NEWLINE`.
    pub(crate) line: usize,
    next_newline: bool,
}

impl Tokenizer {
    /// `Parser::ParseData` appends `'\n'` and `'\0'` before tokenizing.
    pub(crate) fn new(contents: &[u8]) -> Self {
        let mut data = Vec::with_capacity(contents.len() + 2);
        data.extend_from_slice(contents);
        data.push(b'\n');
        data.push(0);
        Self {
            data,
            ptr: 0,
            line: 0,
            next_newline: false,
        }
    }

    fn at(&self, index: usize) -> u8 {
        self.data.get(index).copied().unwrap_or(0)
    }

    pub(crate) fn next_token(&mut self) -> Token {
        if self.next_newline {
            self.next_newline = false;
            return Token::Newline;
        }

        let mut x = self.ptr;
        loop {
            match self.at(x) {
                0 => {
                    self.ptr = x;
                    return Token::Eof;
                }
                b'\n' => {
                    x += 1;
                    self.ptr = x;
                    return Token::Newline;
                }
                b' ' | b'\t' | b'\r' => {
                    x += 1;
                    continue;
                }
                b'#' => {
                    while self.at(x) != 0 && self.at(x) != b'\n' {
                        x += 1;
                    }
                    if self.at(x) == b'\n' {
                        self.ptr = x + 1;
                        return Token::Newline;
                    }
                    self.ptr = x;
                    return Token::Eof;
                }
                _ => break,
            }
        }

        // text:
        let mut text: Vec<u8> = Vec::new();
        loop {
            match self.at(x) {
                0 => break,
                b' ' | b'\t' | b'\r' => {
                    x += 1;
                    break;
                }
                b'\n' => {
                    self.next_newline = true;
                    x += 1;
                    break;
                }
                b'"' => {
                    x += 1;
                    loop {
                        match self.at(x) {
                            0 => {
                                // Unterminated quoted thing.
                                self.ptr = x;
                                return Token::Eof;
                            }
                            b'"' => {
                                x += 1;
                                break;
                            }
                            byte => {
                                text.push(byte);
                                x += 1;
                            }
                        }
                    }
                }
                b'\\' => {
                    x += 1;
                    match self.at(x) {
                        0 => break,
                        b'n' => {
                            text.push(b'\n');
                            x += 1;
                        }
                        b'r' => {
                            text.push(b'\r');
                            x += 1;
                        }
                        b't' => {
                            text.push(b'\t');
                            x += 1;
                        }
                        b'\\' => {
                            text.push(b'\\');
                            x += 1;
                        }
                        b'\r' => {
                            // \ <cr> <lf> -> line continuation
                            if self.at(x + 1) != b'\n' {
                                x += 1;
                                continue;
                            }
                            x += 1;
                            self.continue_line(&mut x);
                        }
                        b'\n' => self.continue_line(&mut x),
                        byte => {
                            // Unknown escape: just copy.
                            text.push(byte);
                            x += 1;
                        }
                    }
                }
                byte => {
                    text.push(byte);
                    x += 1;
                }
            }
        }
        self.ptr = x;
        Token::Text(String::from_utf8_lossy(&text).into_owned())
    }

    /// `\ <lf>`: count the line, skip the newline and any leading blanks.
    fn continue_line(&mut self, x: &mut usize) {
        self.line += 1;
        *x += 1;
        while self.at(*x) == b' ' || self.at(*x) == b'\t' {
            *x += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `RunTest` from system/core/init/tokenizer_test.cpp: every T_NEWLINE
    /// closes a line (even an empty one); T_EOF ends the run.
    fn run(data: &str, expected: &[&[&str]]) {
        let mut tokenizer = Tokenizer::new(data.as_bytes());
        let mut current: Vec<String> = Vec::new();
        let mut tokens: Vec<Vec<String>> = Vec::new();
        loop {
            match tokenizer.next_token() {
                Token::Eof => break,
                Token::Newline => tokens.push(std::mem::take(&mut current)),
                Token::Text(text) => current.push(text),
            }
        }
        let expected: Vec<Vec<String>> = expected
            .iter()
            .map(|line| line.iter().map(|s| s.to_string()).collect())
            .collect();
        assert_eq!(tokens, expected, "{data:?}");
    }

    #[test]
    fn null() {
        run("", &[&[]]);
    }

    #[test]
    fn simple() {
        run("one two\tthree\rfour", &[&["one", "two", "three", "four"]]);
        run(
            "1 2 3\n4 5 6\n7 8 9",
            &[&["1", "2", "3"], &["4", "5", "6"], &["7", "8", "9"]],
        );
        run(
            "    1 2 3\n\t\t\t\t4 5 6\n\r\r\r\r7 8 9",
            &[&["1", "2", "3"], &["4", "5", "6"], &["7", "8", "9"]],
        );
    }

    #[test]
    fn comments() {
        run(
            "1 2 3\n#4 5 6\n7 8 9",
            &[&["1", "2", "3"], &[], &["7", "8", "9"]],
        );
        run(
            "#1 2 3\n4 5 6\n7 8 9",
            &[&[], &["4", "5", "6"], &["7", "8", "9"]],
        );
        run(
            "1 2 3\n4 5 6\n#7 8 9",
            &[&["1", "2", "3"], &["4", "5", "6"], &[]],
        );
        run("1 2 #3\n4 #5 6\n#7 8 9", &[&["1", "2"], &["4"], &[]]);
    }

    #[test]
    fn control_chars() {
        run(r"1 token\ntoken 2", &[&["1", "token\ntoken", "2"]]);
        run(r"1 token\rtoken 2", &[&["1", "token\rtoken", "2"]]);
        run(r"1 token\ttoken 2", &[&["1", "token\ttoken", "2"]]);
        run(r"1 token\\token 2", &[&["1", "token\\token", "2"]]);
        run(r"1 token\btoken 2", &[&["1", "tokenbtoken", "2"]]);
        run(r"1 token\n 2", &[&["1", "token\n", "2"]]);
        run(r"1 token\r 2", &[&["1", "token\r", "2"]]);
        run(r"1 token\t 2", &[&["1", "token\t", "2"]]);
        run(r"1 token\\ 2", &[&["1", "token\\", "2"]]);
        run(r"1 token\b 2", &[&["1", "tokenb", "2"]]);
        run(r"1 \ntoken 2", &[&["1", "\ntoken", "2"]]);
        run(r"1 \btoken 2", &[&["1", "btoken", "2"]]);
        run(r"1 \n 2", &[&["1", "\n", "2"]]);
        run(r"1 \\ 2", &[&["1", "\\", "2"]]);
        run(r"1 \b 2", &[&["1", "b", "2"]]);
    }

    #[test]
    fn cr_lf() {
        run("lf\\\ncont", &[&["lfcont"]]);
        run("lf\\\n    \t\t\t\tcont", &[&["lfcont"]]);
        run("crlf\\\r\ncont", &[&["crlfcont"]]);
        run("crlf\\\r\n    \t\t\t\tcont", &[&["crlfcont"]]);
        run("cr\\\rcont", &[&["crcont"]]);
        run("lfspace \\\ncont", &[&["lfspace", "cont"]]);
        run("lfspace \\\n    \t\t\t\tcont", &[&["lfspace", "cont"]]);
        run("crlfspace \\\r\ncont", &[&["crlfspace", "cont"]]);
        run(
            "crlfspace \\\r\n    \t\t\t\tcont",
            &[&["crlfspace", "cont"]],
        );
        run("crspace \\\rcont", &[&["crspace", "cont"]]);
    }

    #[test]
    fn quoted() {
        run("\"quoted simple string\"", &[&["quoted simple string"]]);
        run("\"unterminated quoted string", &[]);
        run("\"1 2 3\"\n \"unterminated quoted string", &[&["1 2 3"]]);
        run("\"quoted escaped quote\\\"\"", &[]);
        run("\"quoted escaped\\\" quote\"", &[]);
        run("\"\\\"quoted escaped quote\"", &[]);
        run(
            "\"quoted control characters \\n \\r \\t \\\\ \\b \\\r \\\n \r \n\"",
            &[&["quoted control characters \\n \\r \\t \\\\ \\b \\\r \\\n \r \n"]],
        );
        run(
            "\"quoted simple string\" \"second quoted string\"",
            &[&["quoted simple string", "second quoted string"]],
        );
        run(
            "\"# comment quoted string\"",
            &[&["# comment quoted string"]],
        );
        run(
            "\"Adjacent \"\"quoted strings\"",
            &[&["Adjacent quoted strings"]],
        );
    }
}
