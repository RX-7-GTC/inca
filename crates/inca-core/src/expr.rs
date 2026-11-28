//! Small expression evaluator for Maya-style channel expressions.
//!
//! Supports numbers, `time`/`frame`, `pi`, channel names supplied by the caller, arithmetic,
//! parentheses and the usual single-argument maths functions plus `min`, `max` and `pow`.

pub fn eval(src: &str, var: &dyn Fn(&str) -> Option<f64>, time: f64) -> Result<f64, String> {
    let mut p = Parser { s: src.as_bytes(), i: 0, var, time };
    let v = p.expr()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(format!("unexpected '{}'", &src[p.i..]));
    }
    if !v.is_finite() {
        return Err("result is not finite".into());
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    var: &'a dyn Fn(&str) -> Option<f64>,
    time: f64,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn expr(&mut self) -> Result<f64, String> {
        let mut v = self.term()?;
        loop {
            if self.eat(b'+') {
                v += self.term()?;
            } else if self.eat(b'-') {
                v -= self.term()?;
            } else {
                return Ok(v);
            }
        }
    }
    fn term(&mut self) -> Result<f64, String> {
        let mut v = self.power()?;
        loop {
            if self.eat(b'*') {
                v *= self.power()?;
            } else if self.eat(b'/') {
                v /= self.power()?;
            } else if self.eat(b'%') {
                v %= self.power()?;
            } else {
                return Ok(v);
            }
        }
    }
    fn power(&mut self) -> Result<f64, String> {
        let base = self.unary()?;
        if self.eat(b'^') {
            Ok(base.powf(self.power()?))
        } else {
            Ok(base)
        }
    }
    fn unary(&mut self) -> Result<f64, String> {
        if self.eat(b'-') {
            Ok(-self.unary()?)
        } else if self.eat(b'+') {
            self.unary()
        } else {
            self.primary()
        }
    }
    fn primary(&mut self) -> Result<f64, String> {
        self.ws();
        if self.eat(b'(') {
            let v = self.expr()?;
            if !self.eat(b')') {
                return Err("missing ')'".into());
            }
            return Ok(v);
        }
        if self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.') {
            let start = self.i;
            while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.') {
                self.i += 1;
            }
            let text = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| "bad number")?;
            return text.parse::<f64>().map_err(|_| format!("bad number '{text}'"));
        }
        let start = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_') {
            self.i += 1;
        }
        if self.i == start {
            return Err("expected a value".into());
        }
        let name = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| "bad name")?;
        self.ws();
        if self.eat(b'(') {
            let mut args = vec![];
            if !self.eat(b')') {
                loop {
                    args.push(self.expr()?);
                    if self.eat(b')') {
                        break;
                    }
                    if !self.eat(b',') {
                        return Err("expected ',' or ')'".into());
                    }
                }
            }
            return call(name, &args);
        }
        match name {
            "time" | "frame" | "f" => Ok(self.time),
            "pi" => Ok(std::f64::consts::PI),
            _ => (self.var)(name).ok_or_else(|| format!("unknown channel '{name}'")),
        }
    }
}

fn call(name: &str, a: &[f64]) -> Result<f64, String> {
    let one = |f: fn(f64) -> f64| -> Result<f64, String> {
        if a.len() != 1 {
            return Err(format!("{name} expects 1 argument"));
        }
        Ok(f(a[0]))
    };
    match name {
        "sin" => one(f64::sin),
        "cos" => one(f64::cos),
        "tan" => one(f64::tan),
        "abs" => one(f64::abs),
        "sqrt" => one(f64::sqrt),
        "floor" => one(f64::floor),
        "ceil" => one(f64::ceil),
        "round" => one(f64::round),
        "exp" => one(f64::exp),
        "log" => one(f64::ln),
        "min" if a.len() == 2 => Ok(a[0].min(a[1])),
        "max" if a.len() == 2 => Ok(a[0].max(a[1])),
        "pow" if a.len() == 2 => Ok(a[0].powf(a[1])),
        _ => Err(format!("unknown function '{name}' or wrong argument count")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_and_time() {
        let v = |n: &str| match n {
            "translateX" => Some(2.0),
            _ => None,
        };
        assert_eq!(eval("time * 2 + 1", &v, 3.0).unwrap(), 7.0);
        assert_eq!(eval("(translateX + 3) / 2", &v, 0.0).unwrap(), 2.5);
        assert_eq!(eval("sin(pi / 2)", &v, 0.0).unwrap(), 1.0);
        assert!(eval("missing + 1", &v, 0.0).is_err());
    }
}
