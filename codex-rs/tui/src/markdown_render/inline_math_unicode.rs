//! Conservative single-line rendering for simple inline LaTeX.
//!
//! Returning `None` is intentional: formulas that need layout, use an unknown
//! command, or cannot be represented by the small glyph set stay on the RaTeX
//! image path.

pub(super) fn render(formula: &str) -> Option<String> {
    let mut parser = Parser::new(formula.trim());
    let rendered = parser.sequence(/*closing*/ None)?;
    let rendered = rendered.trim();
    (!rendered.is_empty()).then(|| rendered.to_string())
}

struct Parser<'a> {
    source: &'a str,
    offset: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self { source, offset: 0 }
    }

    fn sequence(&mut self, closing: Option<char>) -> Option<String> {
        let mut output = String::new();
        while let Some(character) = self.peek() {
            if Some(character) == closing {
                self.bump();
                return Some(output);
            }
            match character {
                '}' => return None,
                '{' => {
                    self.bump();
                    output.push_str(&self.sequence(Some('}'))?);
                }
                '\\' => {
                    let (command, named_function) = self.command()?;
                    if named_function
                        && output
                            .chars()
                            .next_back()
                            .is_some_and(needs_space_before_named_function)
                    {
                        output.push(' ');
                    }
                    output.push_str(&command);
                    if named_function {
                        loop {
                            while self.peek().is_some_and(char::is_whitespace) {
                                self.bump();
                            }
                            let Some(character @ ('^' | '_')) = self.peek() else {
                                break;
                            };
                            self.bump();
                            output.push_str(&self.script(character == '^')?);
                        }
                        let needs_separator = self
                            .peek()
                            .is_some_and(|character| !matches!(character, '(' | '['));
                        if needs_separator {
                            output.push(' ');
                        }
                    }
                }
                '^' | '_' => {
                    if output.is_empty() {
                        return None;
                    }
                    self.bump();
                    output.push_str(&self.script(character == '^')?);
                }
                '\'' => {
                    let mut count = 0;
                    while self.peek() == Some('\'') {
                        self.bump();
                        count += 1;
                    }
                    output.push_str(match count {
                        1 => "′",
                        2 => "″",
                        3 => "‴",
                        _ => return None,
                    });
                }
                '#' | '$' | '%' | '&' | '~' => return None,
                character if character.is_whitespace() => {
                    self.bump();
                }
                character if !character.is_control() => {
                    self.bump();
                    output.push(character);
                }
                _ => return None,
            }
        }
        closing.is_none().then_some(output)
    }

    fn command(&mut self) -> Option<(String, bool)> {
        self.bump();
        let first = self.bump()?;
        if !first.is_ascii_alphabetic() {
            return control_symbol(first).map(|symbol| (symbol.to_string(), false));
        }

        let start = self.offset - first.len_utf8();
        while self
            .peek()
            .is_some_and(|character| character.is_ascii_alphabetic())
        {
            self.bump();
        }
        let command = &self.source[start..self.offset];
        if matches!(command, "left" | "right") {
            return Some((String::new(), false));
        }
        if matches!(
            command,
            "mathbf" | "mathrm" | "mathit" | "mathsf" | "mathtt" | "boldsymbol"
        ) {
            return Some((self.required_group()?, false));
        }
        if command == "mathbb" {
            let content = self.required_group()?;
            return mathbb(&content).map(|symbol| (symbol.to_string(), false));
        }
        if matches!(command, "frac" | "tfrac") {
            let numerator = self.numeric_argument()?;
            let denominator = self.numeric_argument()?;
            return vulgar_fraction(&numerator, &denominator)
                .map(|fraction| (fraction.to_string(), false));
        }
        if command == "text" {
            return Some((self.text_group()?, false));
        }
        if command == "operatorname" {
            let operator = self.text_group()?;
            return Some((operator, true));
        }
        if let Some(function) = named_function(command) {
            return Some((function.to_string(), true));
        }
        symbol(command).map(|symbol| (symbol.to_string(), false))
    }

    fn required_group(&mut self) -> Option<String> {
        if self.bump()? != '{' {
            return None;
        }
        self.sequence(Some('}'))
    }

    fn numeric_argument(&mut self) -> Option<String> {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
        let argument = if self.peek() == Some('{') {
            self.required_group()?
        } else {
            self.bump()?.to_string()
        };
        (!argument.is_empty() && argument.bytes().all(|byte| byte.is_ascii_digit()))
            .then_some(argument)
    }

    fn text_group(&mut self) -> Option<String> {
        if self.bump()? != '{' {
            return None;
        }
        let mut output = String::new();
        while let Some(character) = self.bump() {
            match character {
                '}' => return Some(output),
                '{' | '\n' | '\r' => return None,
                '\\' => {
                    let escaped = self.bump()?;
                    if matches!(escaped, '{' | '}' | '%' | '#' | '&' | '_' | '\\') {
                        output.push(escaped);
                    } else {
                        return None;
                    }
                }
                character if !character.is_control() => output.push(character),
                _ => return None,
            }
        }
        None
    }

    fn script(&mut self, superscript: bool) -> Option<String> {
        let source = if self.peek() == Some('{') {
            self.bump();
            self.sequence(Some('}'))?
        } else {
            self.bump()?.to_string()
        };
        source
            .chars()
            .map(|character| script_character(character, superscript))
            .collect()
    }

    fn peek(&self) -> Option<char> {
        self.source[self.offset..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.offset += character.len_utf8();
        Some(character)
    }
}

fn needs_space_before_named_function(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, ')' | ']' | '}' | '′' | '″' | '‴')
}

fn vulgar_fraction(numerator: &str, denominator: &str) -> Option<char> {
    match (numerator, denominator) {
        ("1", "2") => Some('½'),
        ("1", "3") => Some('⅓'),
        ("2", "3") => Some('⅔'),
        ("1", "4") => Some('¼'),
        ("3", "4") => Some('¾'),
        ("1", "5") => Some('⅕'),
        ("2", "5") => Some('⅖'),
        ("3", "5") => Some('⅗'),
        ("4", "5") => Some('⅘'),
        ("1", "6") => Some('⅙'),
        ("5", "6") => Some('⅚'),
        ("1", "7") => Some('⅐'),
        ("1", "8") => Some('⅛'),
        ("3", "8") => Some('⅜'),
        ("5", "8") => Some('⅝'),
        ("7", "8") => Some('⅞'),
        ("1", "9") => Some('⅑'),
        ("1", "10") => Some('⅒'),
        _ => None,
    }
}

fn control_symbol(symbol: char) -> Option<&'static str> {
    match symbol {
        ' ' | ',' | ':' | ';' => Some(" "),
        '!' => Some(""),
        '{' => Some("{"),
        '}' => Some("}"),
        '%' => Some("%"),
        '#' => Some("#"),
        '&' => Some("&"),
        '_' => Some("_"),
        '|' => Some("‖"),
        _ => None,
    }
}

fn named_function(command: &str) -> Option<&'static str> {
    match command {
        "arccos" => Some("arccos"),
        "arcsin" => Some("arcsin"),
        "arctan" => Some("arctan"),
        "cos" => Some("cos"),
        "cosh" => Some("cosh"),
        "det" => Some("det"),
        "dim" => Some("dim"),
        "exp" => Some("exp"),
        "gcd" => Some("gcd"),
        "hom" => Some("hom"),
        "inf" => Some("inf"),
        "ker" => Some("ker"),
        "lim" => Some("lim"),
        "ln" => Some("ln"),
        "log" => Some("log"),
        "max" => Some("max"),
        "min" => Some("min"),
        "mod" => Some("mod"),
        "Pr" => Some("Pr"),
        "sin" => Some("sin"),
        "sinh" => Some("sinh"),
        "sup" => Some("sup"),
        "tan" => Some("tan"),
        "tanh" => Some("tanh"),
        _ => None,
    }
}

fn symbol(command: &str) -> Option<&'static str> {
    match command {
        "alpha" => Some("α"),
        "beta" => Some("β"),
        "gamma" => Some("γ"),
        "delta" => Some("δ"),
        "epsilon" => Some("ϵ"),
        "varepsilon" => Some("ε"),
        "zeta" => Some("ζ"),
        "eta" => Some("η"),
        "theta" => Some("θ"),
        "vartheta" => Some("ϑ"),
        "iota" => Some("ι"),
        "kappa" => Some("κ"),
        "varkappa" => Some("ϰ"),
        "lambda" => Some("λ"),
        "mu" => Some("μ"),
        "nu" => Some("ν"),
        "xi" => Some("ξ"),
        "pi" => Some("π"),
        "varpi" => Some("ϖ"),
        "rho" => Some("ρ"),
        "varrho" => Some("ϱ"),
        "sigma" => Some("σ"),
        "varsigma" => Some("ς"),
        "tau" => Some("τ"),
        "upsilon" => Some("υ"),
        "phi" => Some("ϕ"),
        "varphi" => Some("φ"),
        "chi" => Some("χ"),
        "psi" => Some("ψ"),
        "omega" => Some("ω"),
        "Gamma" => Some("Γ"),
        "Delta" => Some("Δ"),
        "Theta" => Some("Θ"),
        "Lambda" => Some("Λ"),
        "Xi" => Some("Ξ"),
        "Pi" => Some("Π"),
        "Sigma" => Some("Σ"),
        "Phi" => Some("Φ"),
        "Psi" => Some("Ψ"),
        "Omega" => Some("Ω"),
        "partial" => Some("∂"),
        "backslash" => Some("\\"),
        "ell" => Some("ℓ"),
        "hbar" => Some("ℏ"),
        "nabla" => Some("∇"),
        "infty" => Some("∞"),
        "emptyset" | "varnothing" => Some("∅"),
        "forall" => Some("∀"),
        "exists" => Some("∃"),
        "neg" | "lnot" => Some("¬"),
        "land" | "wedge" => Some("∧"),
        "lor" | "vee" => Some("∨"),
        "cdot" => Some("·"),
        "times" => Some("×"),
        "div" => Some("÷"),
        "pm" => Some("±"),
        "mp" => Some("∓"),
        "circ" => Some("∘"),
        "le" | "leq" => Some("≤"),
        "ge" | "geq" => Some("≥"),
        "ne" | "neq" => Some("≠"),
        "approx" => Some("≈"),
        "equiv" => Some("≡"),
        "sim" => Some("∼"),
        "cong" => Some("≅"),
        "propto" => Some("∝"),
        "perp" => Some("⊥"),
        "parallel" => Some("∥"),
        "in" => Some("∈"),
        "notin" => Some("∉"),
        "ni" => Some("∋"),
        "subset" => Some("⊂"),
        "supset" => Some("⊃"),
        "subseteq" => Some("⊆"),
        "supseteq" => Some("⊇"),
        "cup" => Some("∪"),
        "cap" => Some("∩"),
        "to" | "rightarrow" => Some("→"),
        "leftarrow" => Some("←"),
        "leftrightarrow" => Some("↔"),
        "mapsto" => Some("↦"),
        "Rightarrow" | "implies" => Some("⇒"),
        "Leftarrow" => Some("⇐"),
        "Leftrightarrow" | "iff" => Some("⇔"),
        "langle" => Some("⟨"),
        "rangle" => Some("⟩"),
        "lfloor" => Some("⌊"),
        "rfloor" => Some("⌋"),
        "lceil" => Some("⌈"),
        "rceil" => Some("⌉"),
        "vert" | "lvert" | "rvert" => Some("|"),
        "Vert" | "lVert" | "rVert" => Some("‖"),
        "ldots" | "dots" => Some("…"),
        "cdots" => Some("⋯"),
        "quad" => Some(" "),
        "qquad" => Some("  "),
        _ => None,
    }
}

fn mathbb(content: &str) -> Option<&'static str> {
    match content {
        "C" => Some("ℂ"),
        "H" => Some("ℍ"),
        "N" => Some("ℕ"),
        "P" => Some("ℙ"),
        "Q" => Some("ℚ"),
        "R" => Some("ℝ"),
        "Z" => Some("ℤ"),
        _ => None,
    }
}

fn script_character(character: char, superscript: bool) -> Option<char> {
    match (superscript, character) {
        (true, '0') => Some('⁰'),
        (true, '1') => Some('¹'),
        (true, '2') => Some('²'),
        (true, '3') => Some('³'),
        (true, '4') => Some('⁴'),
        (true, '5') => Some('⁵'),
        (true, '6') => Some('⁶'),
        (true, '7') => Some('⁷'),
        (true, '8') => Some('⁸'),
        (true, '9') => Some('⁹'),
        (true, '+') => Some('⁺'),
        (true, '-') => Some('⁻'),
        (true, '=') => Some('⁼'),
        (true, '(') => Some('⁽'),
        (true, ')') => Some('⁾'),
        (true, 'i') => Some('ⁱ'),
        (true, 'n') => Some('ⁿ'),
        (false, '0') => Some('₀'),
        (false, '1') => Some('₁'),
        (false, '2') => Some('₂'),
        (false, '3') => Some('₃'),
        (false, '4') => Some('₄'),
        (false, '5') => Some('₅'),
        (false, '6') => Some('₆'),
        (false, '7') => Some('₇'),
        (false, '8') => Some('₈'),
        (false, '9') => Some('₉'),
        (false, '+') => Some('₊'),
        (false, '-') => Some('₋'),
        (false, '=') => Some('₌'),
        (false, '(') => Some('₍'),
        (false, ')') => Some('₎'),
        (false, 'i') => Some('ᵢ'),
        (false, 'j') => Some('ⱼ'),
        (false, 'n') => Some('ₙ'),
        _ => None,
    }
}

#[cfg(test)]
#[path = "inline_math_unicode_tests.rs"]
mod tests;
