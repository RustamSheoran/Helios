use std::collections::HashMap;
use helios_shared::Result;

#[derive(Debug, Clone)]
pub enum Command {
    Simple(SimpleCommand),
    Pipeline(Vec<Command>),
    Redirect(Box<Command>, RedirectOp),
    Background(Box<Command>),
    Subshell(Box<Command>),
    Sequence(Vec<Command>), // Representing cmd1; cmd2
}

#[derive(Debug, Clone)]
pub struct SimpleCommand {
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirectOp {
    Input(String),         // < file
    Output(String),        // > file
    Append(String),        // >> file
    Error(String),         // 2> file
}

/// A token in our shell command lexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Word(String),
    Pipe,          // |
    Amper,         // &
    Semi,          // ;
    LParen,        // (
    RParen,        // )
    Less,          // <
    Greater,       // >
    DGreater,      // >>
    TwoGreater,    // 2>
}

/// The lexer state machine.
pub struct Lexer<'a> {
    input: &'a str,
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            chars: input.chars().peekable(),
        }
    }

    pub fn tokenize(&mut self, aliases: &HashMap<String, String>) -> Result<Vec<Token>> {
        let mut tokens = Vec::new();

        while let Some(&c) = self.chars.peek() {
            if c.is_whitespace() {
                self.chars.next();
                continue;
            }

            match c {
                '|' => {
                    self.chars.next();
                    tokens.push(Token::Pipe);
                }
                '&' => {
                    self.chars.next();
                    tokens.push(Token::Amper);
                }
                ';' => {
                    self.chars.next();
                    tokens.push(Token::Semi);
                }
                '(' => {
                    self.chars.next();
                    tokens.push(Token::LParen);
                }
                ')' => {
                    self.chars.next();
                    tokens.push(Token::RParen);
                }
                '<' => {
                    self.chars.next();
                    tokens.push(Token::Less);
                }
                '>' => {
                    self.chars.next();
                    // Check for >>
                    let mut is_double = false;
                    if let Some(&next_c) = self.chars.peek() {
                        if next_c == '>' {
                            is_double = true;
                        }
                    }
                    if is_double {
                        self.chars.next();
                        tokens.push(Token::DGreater);
                    } else {
                        tokens.push(Token::Greater);
                    }
                }
                '2' => {
                    // Check if 2>
                    let mut is_two_greater = false;
                    let mut iter_clone = self.chars.clone();
                    iter_clone.next(); // Consume '2'
                    if let Some('>') = iter_clone.peek() {
                        is_two_greater = true;
                    }
                    if is_two_greater {
                        self.chars.next(); // Consume '2'
                        self.chars.next(); // Consume '>'
                        tokens.push(Token::TwoGreater);
                    } else {
                        tokens.push(self.read_word()?);
                    }
                }
                _ => {
                    tokens.push(self.read_word()?);
                }
            }
        }

        // Expand aliases on the first token if it is a Word
        if let Some(Token::Word(first_word)) = tokens.first() {
            if let Some(expanded) = aliases.get(first_word) {
                // Lex expanded string and substitute
                let mut sub_lexer = Lexer::new(expanded);
                let mut expanded_tokens = sub_lexer.tokenize(aliases)?;
                expanded_tokens.extend(tokens.into_iter().skip(1));
                tokens = expanded_tokens;
            }
        }

        Ok(tokens)
    }

    fn read_word(&mut self) -> Result<Token> {
        let mut word = String::new();
        let mut in_double_quotes = false;
        let mut in_single_quotes = false;

        while let Some(&c) = self.chars.peek() {
            if in_single_quotes {
                self.chars.next();
                if c == '\'' {
                    in_single_quotes = false;
                } else {
                    word.push(c);
                }
            } else if in_double_quotes {
                self.chars.next();
                if c == '"' {
                    in_double_quotes = false;
                } else if c == '\\' {
                    if let Some(next_c) = self.chars.next() {
                        word.push(next_c);
                    }
                } else if c == '$' {
                    let env_val = self.read_env_var()?;
                    word.push_str(&env_val);
                } else {
                    word.push(c);
                }
            } else {
                if c.is_whitespace() || matches!(c, '|' | '&' | ';' | '(' | ')' | '<' | '>') {
                    break;
                }
                self.chars.next();
                if c == '\'' {
                    in_single_quotes = true;
                } else if c == '"' {
                    in_double_quotes = true;
                } else if c == '\\' {
                    if let Some(next_c) = self.chars.next() {
                        word.push(next_c);
                    }
                } else if c == '$' {
                    let expanded = self.read_env_var()?;
                    word.push_str(&expanded);
                } else {
                    word.push(c);
                }
            }
        }

        Ok(Token::Word(word))
    }

    fn read_env_var(&mut self) -> Result<String> {
        let mut var_name = String::new();
        while let Some(&c) = self.chars.peek() {
            if c.is_alphanumeric() || c == '_' {
                self.chars.next();
                var_name.push(c);
            } else {
                break;
            }
        }
        if var_name.is_empty() {
            return Ok("$".to_string()); // Raw literal $
        }
        // Expand environment variable
        let val = std::env::var(&var_name).unwrap_or_default();
        Ok(val)
    }
}

/// The command AST parser.
pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub fn parse(&mut self) -> Result<Command> {
        self.parse_sequence()
    }

    // Sequence parsing: cmd1; cmd2; cmd3
    fn parse_sequence(&mut self) -> Result<Command> {
        let mut cmds = Vec::new();
        let mut first = self.parse_background()?;
        
        while self.peek() == Some(&Token::Semi) {
            self.consume();
            cmds.push(first);
            if self.is_eof() {
                // Trailing semicolon
                first = Command::Sequence(cmds);
                return Ok(first);
            }
            first = self.parse_background()?;
        }

        if !cmds.is_empty() {
            cmds.push(first);
            Ok(Command::Sequence(cmds))
        } else {
            Ok(first)
        }
    }

    // Background parsing: cmd &
    fn parse_background(&mut self) -> Result<Command> {
        let cmd = self.parse_pipeline()?;
        if self.peek() == Some(&Token::Amper) {
            self.consume();
            Ok(Command::Background(Box::new(cmd)))
        } else {
            Ok(cmd)
        }
    }

    // Pipeline parsing: cmd1 | cmd2 | cmd3
    fn parse_pipeline(&mut self) -> Result<Command> {
        let first = self.parse_redirect()?;
        let mut pipeline = Vec::new();

        while self.peek() == Some(&Token::Pipe) {
            self.consume();
            if pipeline.is_empty() {
                pipeline.push(first.clone());
            }
            let next_cmd = self.parse_redirect()?;
            pipeline.push(next_cmd);
        }

        if !pipeline.is_empty() {
            Ok(Command::Pipeline(pipeline))
        } else {
            Ok(first)
        }
    }

    // Redirections parsing: cmd > file < input >> append 2> err
    fn parse_redirect(&mut self) -> Result<Command> {
        let mut cmd = self.parse_subshell_or_simple()?;

        loop {
            match self.peek() {
                Some(&Token::Less) => {
                    self.consume();
                    if let Some(Token::Word(file)) = self.consume_clone() {
                        cmd = Command::Redirect(Box::new(cmd), RedirectOp::Input(file));
                    } else {
                        return Err(helios_shared::HeliosError::ParserError(
                            "Expected filename after '<' redirection operator".to_string(),
                        ));
                    }
                }
                Some(&Token::Greater) => {
                    self.consume();
                    if let Some(Token::Word(file)) = self.consume_clone() {
                        cmd = Command::Redirect(Box::new(cmd), RedirectOp::Output(file));
                    } else {
                        return Err(helios_shared::HeliosError::ParserError(
                            "Expected filename after '>' redirection operator".to_string(),
                        ));
                    }
                }
                Some(&Token::DGreater) => {
                    self.consume();
                    if let Some(Token::Word(file)) = self.consume_clone() {
                        cmd = Command::Redirect(Box::new(cmd), RedirectOp::Append(file));
                    } else {
                        return Err(helios_shared::HeliosError::ParserError(
                            "Expected filename after '>>' redirection operator".to_string(),
                        ));
                    }
                }
                Some(&Token::TwoGreater) => {
                    self.consume();
                    if let Some(Token::Word(file)) = self.consume_clone() {
                        cmd = Command::Redirect(Box::new(cmd), RedirectOp::Error(file));
                    } else {
                        return Err(helios_shared::HeliosError::ParserError(
                            "Expected filename after '2>' redirection operator".to_string(),
                        ));
                    }
                }
                _ => break,
            }
        }

        Ok(cmd)
    }

    // Subshell or Simple command parsing
    fn parse_subshell_or_simple(&mut self) -> Result<Command> {
        if self.peek() == Some(&Token::LParen) {
            self.consume();
            let sub_cmd = self.parse_sequence()?;
            if self.peek() == Some(&Token::RParen) {
                self.consume();
                Ok(Command::Subshell(Box::new(sub_cmd)))
            } else {
                Err(helios_shared::HeliosError::ParserError(
                    "Expected matching ')' to close subshell block".to_string(),
                ))
            }
        } else {
            self.parse_simple()
        }
    }

    fn parse_simple(&mut self) -> Result<Command> {
        let mut args = Vec::new();
        while let Some(Token::Word(arg)) = self.peek_clone() {
            self.consume();
            args.push(arg);
        }

        if args.is_empty() {
            // Check if we hit parsing blockers
            if self.is_eof() || matches!(self.peek(), Some(&Token::RParen) | Some(&Token::Semi) | Some(&Token::Pipe)) {
                return Ok(Command::Simple(SimpleCommand { args: Vec::new() }));
            }
            return Err(helios_shared::HeliosError::ParserError(
                "Expected command name, found operator".to_string(),
            ));
        }

        Ok(Command::Simple(SimpleCommand { args }))
    }

    // Helper functions
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_clone(&self) -> Option<Token> {
        self.tokens.get(self.pos).cloned()
    }

    fn consume(&mut self) {
        self.pos += 1;
    }

    fn consume_clone(&mut self) -> Option<Token> {
        let tok = self.peek_clone();
        self.consume();
        tok
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.tokens.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_lexer_tokenization() {
        let aliases = HashMap::new();
        let mut lexer = Lexer::new("ls -la | grep cargo > output.txt &");
        let tokens = lexer.tokenize(&aliases).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Word("ls".to_string()),
                Token::Word("-la".to_string()),
                Token::Pipe,
                Token::Word("grep".to_string()),
                Token::Word("cargo".to_string()),
                Token::Greater,
                Token::Word("output.txt".to_string()),
                Token::Amper,
            ]
        );
    }

    #[test]
    fn test_quotes_preservation() {
        let aliases = HashMap::new();
        let mut lexer = Lexer::new("echo \"hello world\" 'single quote'");
        let tokens = lexer.tokenize(&aliases).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".to_string()),
                Token::Word("hello world".to_string()),
                Token::Word("single quote".to_string()),
            ]
        );
    }

    #[test]
    fn test_alias_expansion() {
        let mut aliases = HashMap::new();
        aliases.insert("ll".to_string(), "ls -l".to_string());

        let mut lexer = Lexer::new("ll -a");
        let tokens = lexer.tokenize(&aliases).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Word("ls".to_string()),
                Token::Word("-l".to_string()),
                Token::Word("-a".to_string()),
            ]
        );
    }

    #[test]
    fn test_parser_grammar() {
        let aliases = HashMap::new();
        let mut lexer = Lexer::new("cat input.txt | grep error > out.log");
        let tokens = lexer.tokenize(&aliases).unwrap();

        let mut parser = Parser::new(tokens);
        let cmd = parser.parse().unwrap();

        // The parser should construct a Pipeline of length 2
        if let Command::Pipeline(pipeline) = cmd {
            assert_eq!(pipeline.len(), 2);
            
            // First pipeline stage: cat input.txt
            if let Command::Simple(ref first) = pipeline[0] {
                assert_eq!(first.args, vec!["cat".to_string(), "input.txt".to_string()]);
            } else {
                panic!("Expected simple command first in pipeline");
            }

            // Second pipeline stage: grep error > out.log
            if let Command::Redirect(ref inner, RedirectOp::Output(ref file)) = pipeline[1] {
                assert_eq!(file, "out.log");
                if let Command::Simple(ref second) = **inner {
                    assert_eq!(second.args, vec!["grep".to_string(), "error".to_string()]);
                } else {
                    panic!("Expected simple command inside redirected stage");
                }
            } else {
                panic!("Expected redirection on the second stage of pipeline");
            }
        } else {
            panic!("Expected pipeline root command");
        }
    }
}
