use slime_proto::spawn::{MAX_ARGUMENT_BYTES, MAX_ARGUMENTS, MAX_ENVIRONMENT_BYTES};

pub const MAX_LINE_BYTES: usize = 128;
const MAX_DEPTH: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    Empty,
    Expected(char),
    ExpectedCommand,
    ExpectedContextValue,
    TooDeep,
    TooManyArguments,
    ArgumentTooLong,
    EnvironmentTooLong,
    UnsupportedForm,
    UnsupportedShellToken,
    TrailingInput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackedFields<const N: usize> {
    pub count: u8,
    pub bytes: [u8; N],
}

impl<const N: usize> PackedFields<N> {
    const fn empty() -> Self {
        Self {
            count: 0,
            bytes: [0; N],
        }
    }

    fn push(&mut self, value: &[u8], max_len: usize) -> Result<(), ParseError> {
        if value.is_empty() || value.len() > max_len {
            return Err(ParseError::ArgumentTooLong);
        }
        let mut offset = 0;
        for _ in 0..self.count {
            offset += 1 + self.bytes[offset] as usize;
        }
        if offset + 1 + value.len() > N {
            return Err(ParseError::ArgumentTooLong);
        }
        self.bytes[offset] = value.len() as u8;
        self.bytes[offset + 1..offset + 1 + value.len()].copy_from_slice(value);
        self.count += 1;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Launch<'a> {
    pub command: &'a [u8],
    pub arguments: PackedFields<8>,
    pub environment: PackedFields<8>,
    pub cwd: Option<&'a [u8]>,
    pub stdin: Option<&'a [u8]>,
}

pub fn parse(input: &[u8]) -> Result<Launch<'_>, ParseError> {
    let mut parser = Parser { input, cursor: 0 };
    parser.space();
    if parser.cursor == input.len() {
        return Err(ParseError::Empty);
    }
    let mut context = Context::empty();
    let launch = parser.expression(&mut context, 0)?;
    parser.space();
    if parser.cursor != input.len() {
        return Err(ParseError::TrailingInput);
    }
    Ok(launch)
}

#[derive(Clone, Copy)]
struct Context<'a> {
    environment: PackedFields<8>,
    cwd: Option<&'a [u8]>,
    stdin: Option<&'a [u8]>,
}

impl Context<'_> {
    const fn empty() -> Self {
        Self {
            environment: PackedFields::empty(),
            cwd: None,
            stdin: None,
        }
    }
}

struct Parser<'a> {
    input: &'a [u8],
    cursor: usize,
}

impl<'a> Parser<'a> {
    fn expression(
        &mut self,
        context: &mut Context<'a>,
        depth: usize,
    ) -> Result<Launch<'a>, ParseError> {
        if depth >= MAX_DEPTH {
            return Err(ParseError::TooDeep);
        }
        self.space();
        if self.consume(b"$(") {
            return self.shell(*context);
        }
        if !self.consume(b"(") {
            return Err(ParseError::UnsupportedForm);
        }
        let form = self.token()?;
        match form {
            b"with-env" => {
                self.space();
                self.expect(b'{', '{')?;
                self.space();
                let value = self.token()?;
                context.environment = PackedFields::empty();
                context
                    .environment
                    .push(value, MAX_ENVIRONMENT_BYTES)
                    .map_err(|_| ParseError::EnvironmentTooLong)?;
                self.space();
                self.expect(b'}', '}')?;
                let launch = self.expression(context, depth + 1)?;
                self.space();
                self.expect(b')', ')')?;
                Ok(launch)
            }
            b"with-cwd" => {
                self.space();
                context.cwd = Some(self.token()?);
                if context.cwd == Some(b"_") {
                    return Err(ParseError::ExpectedContextValue);
                }
                let launch = self.expression(context, depth + 1)?;
                self.space();
                self.expect(b')', ')')?;
                Ok(launch)
            }
            b"with-stdin" => {
                self.space();
                let value = self.token()?;
                if value == b"_" {
                    return Err(ParseError::ExpectedContextValue);
                }
                context.stdin = Some(value);
                let launch = self.expression(context, depth + 1)?;
                self.space();
                self.expect(b')', ')')?;
                Ok(launch)
            }
            _ => Err(ParseError::UnsupportedForm),
        }
    }

    fn shell(&mut self, context: Context<'a>) -> Result<Launch<'a>, ParseError> {
        self.space();
        let command = self.token()?;
        if command.is_empty() {
            return Err(ParseError::ExpectedCommand);
        }
        let mut arguments = PackedFields::empty();
        loop {
            self.space();
            if self.consume(b")") {
                break;
            }
            if arguments.count as usize >= MAX_ARGUMENTS {
                return Err(ParseError::TooManyArguments);
            }
            let argument = self.token()?;
            if argument.starts_with(b"$")
                || argument
                    .iter()
                    .any(|byte| matches!(byte, b'*' | b'?' | b'[' | b']' | b'|'))
            {
                return Err(ParseError::UnsupportedShellToken);
            }
            arguments.push(argument, MAX_ARGUMENT_BYTES)?;
        }
        Ok(Launch {
            command,
            arguments,
            environment: context.environment,
            cwd: context.cwd,
            stdin: context.stdin,
        })
    }

    fn token(&mut self) -> Result<&'a [u8], ParseError> {
        self.space();
        let start = self.cursor;
        while let Some(byte) = self.input.get(self.cursor).copied() {
            if byte.is_ascii_whitespace() || matches!(byte, b'(' | b')' | b'{' | b'}') {
                break;
            }
            self.cursor += 1;
        }
        if start == self.cursor {
            return Err(ParseError::ExpectedContextValue);
        }
        Ok(&self.input[start..self.cursor])
    }

    fn space(&mut self) {
        while self
            .input
            .get(self.cursor)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.cursor += 1;
        }
    }

    fn consume(&mut self, token: &[u8]) -> bool {
        if self.input[self.cursor..].starts_with(token) {
            self.cursor += token.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, byte: u8, expected: char) -> Result<(), ParseError> {
        if self.input.get(self.cursor) == Some(&byte) {
            self.cursor += 1;
            Ok(())
        } else {
            Err(ParseError::Expected(expected))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_launch_and_nested_explicit_context() {
        let launch = parse(b"(with-env {MODE=ci} (with-cwd docs (with-stdin data $(echo ok))))")
            .expect("nested context");
        assert_eq!(launch.command, b"echo");
        assert_eq!(launch.arguments.count, 1);
        assert_eq!(launch.arguments.bytes, [2, b'o', b'k', 0, 0, 0, 0, 0]);
        assert_eq!(launch.environment.count, 1);
        assert_eq!(
            launch.environment.bytes,
            [7, b'M', b'O', b'D', b'E', b'=', b'c', b'i']
        );
        assert_eq!(launch.cwd, Some(b"docs".as_slice()));
        assert_eq!(launch.stdin, Some(b"data".as_slice()));
    }

    #[test]
    fn default_context_is_empty_and_non_ambient() {
        let launch = parse(b"$(sysinfo)").expect("plain launch");
        assert_eq!(launch.arguments.count, 0);
        assert_eq!(launch.environment.count, 0);
        assert_eq!(launch.cwd, None);
        assert_eq!(launch.stdin, None);
    }

    #[test]
    fn rejects_injection_unbounded_and_malformed_forms() {
        for input in [
            b"$(echo $name)".as_slice(),
            b"$(echo a b c)",
            b"$(echo a|b)",
            b"(with-cwd _ $(echo))",
            b"(with-env {TOO-LONG} $(echo))",
            b"$(echo",
        ] {
            assert!(parse(input).is_err(), "accepted {:?}", input);
        }
    }
}
