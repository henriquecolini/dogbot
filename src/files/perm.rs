use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Who {
    User,
    Group,
    Other,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Add,
    Remove,
    Set,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flags {
    pub read: bool,
    pub write: bool,
    pub exec: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Clause {
    pub who: Vec<Who>,
    pub op: Op,
    pub perm: Flags,
}

impl FromStr for Clause {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut chars = s.chars().peekable();

        // --- who ---
        let mut who = vec![];
        while let Some(&c) = chars.peek() {
            match c {
                'u' => who.push(Who::User),
                'g' => who.push(Who::Group),
                'o' => who.push(Who::Other),
                'a' => who.push(Who::All),
                '+' | '-' | '=' => break,
                _ => return Err(format!("invalid who: {}", c)),
            }
            chars.next();
        }

        if who.is_empty() {
            who.push(Who::All); // chmod default
        }

        // --- op ---
        let op = match chars.next() {
            Some('+') => Op::Add,
            Some('-') => Op::Remove,
            Some('=') => Op::Set,
            _ => return Err("expected operator (+, -, =)".into()),
        };

        // --- perms ---
        let mut perm = Flags {
            read: false,
            write: false,
            exec: false,
        };
        for c in chars {
            match c {
                'r' => perm.read = true,
                'w' => perm.write = true,
                'x' => perm.exec = true,
                _ => return Err(format!("invalid perm: {}", c)),
            }
        }

        Ok(Clause { who, op, perm })
    }
}