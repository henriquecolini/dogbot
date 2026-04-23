use crate::prelude::ChatId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChatAddress<'a> {
    Id(ChatId),
    Alias(&'a str),
}

impl ChatAddress<'_> {
    pub fn parse(address: &str) -> ChatAddress<'_> {
        if let Ok(id) = address.parse::<i64>() {
            ChatAddress::Id(ChatId(id))
        } else {
            ChatAddress::Alias(address)
        }
    }
}

impl std::fmt::Display for ChatAddress<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatAddress::Id(id) => write!(f, "{}", id),
            ChatAddress::Alias(alias) => write!(f, "{}", alias),
        }
    }
}
