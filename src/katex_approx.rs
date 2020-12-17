use cursive::utils::markup::StyledString;

#[derive(Debug)]
pub enum KatexError {}

#[derive(Debug)]
pub struct KatexOptions {
    /// Whether or not we should use unicode symbols for katex
    /// Without this, output is a lot more limited in what it can emulate
    pub unicode: bool,
    /// What the code is wrapped in
    /// For now doesn't support specifying a custom double-enclosure.
    pub enclosure: char,
}
impl Default for KatexOptions {
    fn default() -> Self {
        Self {
            unicode: true,
            enclosure: '$',
        }
    }
}

// \frac{a}{b} -> (a/b) (for certain values, there are unicode characters for this.)
// x^2 -> x^2 (might be able to find unicode characters but that would have to be an option)
// x^{25} -> x^{25}
// x_5 -> ?
// \lim \sin \cos etc, could just be made bold?
// \theta \delta \Delta has a unicode
// \R \N \Z \Q, etc could probably be written with unicode. Italic if no unicode?

pub fn convert_to_approximate(
    text: &str,
    options: KatexOptions,
) -> Result<StyledString, KatexError> {
    // let mut iter = text.char_indices().peekable();
    // let mut styled = StyledString::new();

    // // The active span
    // let mut span: Range<usize> = 0..0;
    // while let Some((i, ch)) = iter.next() {
    //     if ch == options.enclosure {
    //         if let Some((i_next, ch_next)) = iter.peek() {
    //         } else {
    //             // EOF, so we just print the $
    //             span.end = i;
    //         }
    //     } else {
    //         span.end = i;
    //     }
    //     match ch {
    //         '$' => {
    //             match iter.peek() {
    //                 Some((i_next, '$')) => {
    //                     // Consume $
    //                     debug_assert_eq!(iter.next().is_some());
    //                 }
    //                 // EOF, so we just print the $
    //                 None => span.end = i,
    //             }
    //         }
    //         _ => span.end = i,
    //     }
    // }

    Ok(StyledString::from(text.to_owned()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_conversion() {}
}
