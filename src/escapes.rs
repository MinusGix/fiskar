use std::{borrow::Cow, collections::HashMap};

use cursive::{
    theme::Effect,
    views::{Dialog, TextView},
};

use crate::styled::{self, StyledIndexedSpan, StyledString};

#[derive(Debug, Clone)]
pub struct Escapes<'a> {
    /// Mapping of thing to replace with what to replace it with.
    pub escapes: HashMap<Cow<'a, str>, Cow<'a, str>>,
}
impl<'a> Escapes<'a> {
    pub fn new() -> Self {
        Self {
            escapes: HashMap::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            escapes: HashMap::with_capacity(capacity),
        }
    }

    /// Applies escapes to text.
    pub fn apply<S>(&self, text: S) -> Escaped<StyledString>
    where
        S: Into<StyledString>,
    {
        let mut styled: StyledString = text.into();
        for (value, escape) in self.escapes.iter() {
            // TODO: we can apply extra styling by using match_indices before modifying it? that
            // wouldn't work
            // TODO: It would be nice to make replaced things styled.. this is in part implemented
            // but full implementation is a pain.
            let new_styled = styled.replace_styled(value.as_ref(), escape.as_ref());
            // for (from, to) in styled.match_replaced_indices(value.as_ref(), escape.as_ref()) {
            //     if !to.is_empty() {
            //         new_styled.add_span_intersect(StyledIndexedSpan::new_range(
            //             to,
            //             Effect::Underline.into(),
            //         ))
            //     }
            // }
            styled = new_styled;
        }
        Escaped(styled)
    }

    pub fn add<S, V>(&mut self, value: V, escape: S)
    where
        S: Into<Cow<'a, str>>,
        V: Into<Cow<'a, str>>,
    {
        self.escapes.insert(value.into(), escape.into());
    }
}
impl<'a> Default for Escapes<'a> {
    fn default() -> Self {
        let mut escapes = Escapes::with_capacity(16);
        escapes.add("\0", "\\0");
        escapes.add("\x01", "\\1");
        escapes
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct Escaped<T>(T);
impl<T> Escaped<T> {
    pub fn into_inner(self) -> T {
        self.0
    }

    pub fn inner(&self) -> &T {
        &self.0
    }
}
impl<T> Escaped<T>
where
    T: AsRef<str>,
{
    pub fn inner_str(&self) -> &str {
        self.0.as_ref()
    }
}

pub fn create_info_dialog<T>(text: Escaped<T>) -> Dialog
where
    T: Into<StyledString>,
{
    Dialog::info(text.into_inner().into())
}

pub fn create_text_view<T>(text: Escaped<T>) -> TextView
where
    T: Into<StyledString>,
{
    TextView::new(text.into_inner().into())
}
