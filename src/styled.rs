use std::ops::Range;

use cursive::{
    theme::Style,
    utils::markup::{
        StyledIndexedSpan as CursiveStyledIndexedSpan, StyledString as CursiveStyledString,
    },
};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum InsertMode {
    /// Break apart anything it was inserted in the middle of, so the inserted text
    /// does not gain the style.
    BreakApart,
    /// Extend any style it intersects with to contain it.
    Extend,
}

/// This is intended to be a workable replacement (more builder) for cursive's styled strings.
/// This is simpler, provides more features (replace), and does away with complex parts (Cow'd
/// spans) that provide little benefit to me and make writing utilities (like replace) way harder.
#[derive(Debug, Clone, PartialEq)]
pub struct StyledString {
    source: String,
    spans: Vec<StyledIndexedSpan>,
}
impl<S> From<S> for StyledString
where
    S: Into<String>,
{
    fn from(value: S) -> Self {
        Self {
            source: value.into(),
            spans: Vec::new(),
        }
    }
}
impl Default for StyledString {
    fn default() -> Self {
        Self {
            source: String::new(),
            spans: Vec::new(),
        }
    }
}
impl StyledString {
    pub fn with_spans<S>(source: S, spans: Vec<StyledIndexedSpan>) -> Self
    where
        S: Into<String>,
    {
        let source = source.into();

        // Check that spans are within bounds.
        for span in spans.iter() {
            assert!(span.range.end <= source.len());
        }

        Self { source, spans }
    }

    pub fn single_span<S>(source: S, attr: Style) -> Self
    where
        S: Into<String>,
    {
        let source = source.into();
        let spans = vec![StyledIndexedSpan::new(&source, attr)];
        Self::with_spans(source, spans)
    }

    pub fn spans_at(&self, idx: usize) -> impl Iterator<Item = &StyledIndexedSpan> {
        self.spans.iter().filter(move |span| span.contains(idx))
    }

    pub fn intersecting_spans(
        &self,
        range: Range<usize>,
    ) -> impl Iterator<Item = &StyledIndexedSpan> {
        self.spans
            .iter()
            .filter(move |span| range_intersection(span.range.clone(), range.clone()).is_some())
    }

    pub fn len(&self) -> usize {
        self.source.len()
    }

    pub fn is_empty(&self) -> bool {
        self.source.is_empty()
    }

    pub fn insert_str(&mut self, idx: usize, text: &str, mode: InsertMode) {
        self.source.insert_str(idx, text);

        let mut found_first_intersection = false;
        let mut spans = Vec::with_capacity(self.spans.len() + 2);
        std::mem::swap(&mut spans, &mut self.spans);
        for mut span in spans {
            if found_first_intersection {
                match mode {
                    InsertMode::BreakApart => {
                        span.offset(text.len());
                        self.spans.push(span);
                    }
                    _ => panic!("Unsupported"),
                }
            } else if span.contains(idx) {
                found_first_intersection = true;
                match mode {
                    InsertMode::BreakApart => {
                        let (left_span, right_span) = span.split_at(idx);
                        if let Some(left_span) = left_span {
                            // The span before the text
                            self.spans.push(left_span);
                        }

                        if let Some(mut right_span) = right_span {
                            // Push the right span so it after the text
                            right_span.offset(text.len());
                            self.spans.push(right_span);
                        }
                    }
                    _ => panic!("Unsupported"),
                }
            } else {
                self.spans.push(span);
            }
        }
    }

    pub fn add_span_intersect(&mut self, new_span: StyledIndexedSpan) {
        if new_span.is_empty() {
            return;
        }

        // Take ownership of all the spans, because we are doing complex breaking up of spans
        // and so we have to decide what spans live or die or are broken into smaller spans.
        let mut spans = Vec::new();
        std::mem::swap(&mut spans, &mut self.spans);

        let mut resulting_spans = Vec::with_capacity(spans.len() + 1);
        // The amount of new_span that has been used in intersections
        // Later used to determine ho wmuch of new_span should be added to the stylings.
        let mut used_new_span = new_span.range.start..new_span.range.start;
        // We iterate over all existing spans, modifying them if they intersect.
        for span in spans {
            // Get the intersection between the two spans.
            // Since cursive's span does not support intersection in a sane manner, we remove
            // intersections at this stage
            let intersection = span.intersection(new_span.range.clone());
            if let Some(intersection) = intersection {
                used_new_span.end = used_new_span.end.max(intersection.end);

                // So we have an intersection between this span and new_span.
                // Get the span with the intersection removed.
                let (orig_left, orig_right) =
                    range_remove(span.range.clone(), intersection.clone());
                if let Some(orig_left) = orig_left {
                    // So, we know that orig_left is the first one, if it exists.
                    // Thus we can just add it so that the order is kept
                    resulting_spans.push(StyledIndexedSpan::new_range(orig_left, span.attr));
                }

                // Combined style so that we can have multiple styles at once.
                let combined_style = span.attr.combine(new_span.attr);
                // Add the intersection which would be directly after orig_left (if that exists)
                let combined_span = StyledIndexedSpan::new_range(intersection, combined_style);
                resulting_spans.push(combined_span);

                if let Some(orig_right) = orig_right {
                    // We know that orig_right is after the constructed span if it exists
                    // Thus we can just add so that is kept.
                    resulting_spans.push(StyledIndexedSpan::new_range(orig_right, span.attr));
                }
            } else {
                // There was no intersection so we just add the span to the resulting spans
                resulting_spans.push(span);
            }
        }

        let (new_left, new_right) = range_remove(new_span.range.clone(), used_new_span);
        if let Some(new_left) = new_left {
            // This maybe should not happen.
            // Pretty sure it _definitely_ should not happen if there is also a new_right.
            resulting_spans.push(StyledIndexedSpan::new_range(new_left, new_span.attr));
        }

        if let Some(new_right) = new_right {
            resulting_spans.push(StyledIndexedSpan::new_range(new_right, new_span.attr));
        }

        self.spans = resulting_spans;
    }

    pub fn append<S>(&mut self, other: S)
    where
        S: Into<StyledString>,
    {
        let other = other.into();
        self.append_source(&other.source);
        self.append_owned_spans(other.spans);
    }

    pub fn append_styled(&mut self, other: &str, style: Style) {
        let start = self.source.len();
        let end = other.len() + start;
        self.append_source(other);
        self.spans
            .push(StyledIndexedSpan::new_range(start..end, style));
    }

    pub fn append_source(&mut self, source: &str) {
        self.source += source;
    }

    fn append_owned_spans(&mut self, mut spans: Vec<StyledIndexedSpan>) {
        let offset = self.source.len();
        for span in spans.iter_mut() {
            span.offset(offset);
        }
        self.spans.append(&mut spans);
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn spans(&self) -> &[StyledIndexedSpan] {
        &self.spans
    }

    // TODO: this could use Pattern once it is stableized
    /// Replaces text content within, but does not keep _any_ styles.
    pub fn simple_replace(&self, from: &str, to: &str) -> StyledString {
        let mut result = StyledString::default();
        let mut last_end = 0;
        for (start, part) in self.source.match_indices(from) {
            // Simplest case, unstyled.
            let before_content = &self.source[last_end..start];
            let new_content = to;
            result.append_source(before_content);
            result.append_source(new_content);
            // Len is number of bytes, so this is presumably accurate.
            last_end = start + part.len();
        }
        result.append_source(&self.source[last_end..self.source.len()]);
        result
    }

    pub fn match_replaced_indices<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
    ) -> impl Iterator<Item = (Range<usize>, Range<usize>)> + 'a {
        // The length of the thing we're matching against
        let match_byte_count = from.len();
        // The length of the thing we're replacing it with.
        let replace_byte_count = to.len();
        self.source
            .match_indices(from)
            .enumerate()
            .map(move |(i, (start, _))| {
                // The range of the text we're 'replacing'/matching-for
                let from_range: Range<usize> = start..(start.saturating_add(match_byte_count));
                let to_range: Range<usize> = {
                    // The number of bytes (of our match) that we've hit so far.
                    // Since we enumerate, `i` is the amount of times we've ran and so the amount of
                    // times we've found ourself.
                    let matched_bytes = i.saturating_mul(match_byte_count);
                    // The number of bytes that we've replaced so far.
                    let replaced_bytes = i.saturating_mul(replace_byte_count);
                    // We subtract the number of matched bytes, so this is the offset without the
                    // matched bytes
                    // Then we add the number of replaced bytes, thus getting our actaul start.
                    let new_start = start
                        .saturating_sub(matched_bytes)
                        .saturating_add(replaced_bytes);

                    let new_end = new_start.saturating_add(replace_byte_count);
                    new_start..new_end
                };
                (from_range, to_range)
            })
    }

    pub fn replace(&self, from: &str, to: &str) -> StyledString {
        self.replace_styled(from, to)
    }

    fn map_styles(&self, from: &str, to: &str) -> Vec<StyledIndexedSpan> {
        let mut spans = self.spans.clone();
        if from.len() == to.len() {
            // We don't have to bother doing anything with this as we know it is already valid
            return spans;
        }

        // At no point does this need to add new spans.
        // Now, it might need to remove *empty* spans, but that can be done later
        for (from_range, to_range) in self.match_replaced_indices(from, to) {}

        spans = spans.into_iter().filter(|span| !span.is_empty()).collect();

        spans
    }

    /// Replace text content within, trying to keep styles.
    pub fn replace_styled(&self, from: &str, to: &str) -> StyledString {
        // The resulting string
        // We expect simple_replace to result in a string without any spans.
        let mut result = self.simple_replace(from, to);
        let mut spans = self.spans.clone();

        // The length of the thing we're matching against
        let match_byte_count = from.len();
        // The length of the thing we're replacing it with
        let replace_byte_count = to.len();

        // for (i, (from_range, _to_range)) in self.match_replaced_indices(from, to) {}

        // for (from_range, to_range) in self.match_replaced_indices(from, to) {
        //     let mut found_first_intersecting = false;
        //     // TODO: we really need to offset the from and to ranges after the first iteration.
        //     // use enumerate or something here to calculate appropraite sbutraction offsets.
        //     for span in spans.iter_mut() {
        //         if found_first_intersecting {
        //             // We just subtract the offsets, moving them back.
        //             span.range = range_subtract(span.range.clone(), match_byte_count);
        //             span.range = range_add(span.range.clone(), replace_byte_count);
        //         } else if let Some(intersection) =
        //             range_intersection(span.range.clone(), from_range.clone())
        //         {
        //             found_first_intersecting = true;
        //             span.range.end -= intersection.len();
        //             span.range.end += replace_byte_count;
        //         }
        //         // otherwise, we don't need to do any modifications as the replacement is after
        //         // this span
        //     }
        // }
        result.spans = spans;
        result
    }
}

impl Into<CursiveStyledString> for StyledString {
    fn into(self) -> CursiveStyledString {
        let source = self.source;

        // This gives everything plaintext spans if it is not already styled.
        // because, the cursive styledstring does not display anything not covered
        // by a span.
        // As well, it will redisplay things that are covered by multiple spans, so
        // we can't simply have a plain-styled span covering everything that is then
        // overwritten by more specific spans.

        // This holds plaintext spans for everyhthing which is not yet styled.
        let mut spans = Vec::with_capacity(16);
        let plain_style = Style::default();
        let mut last_position = 0;
        for span in self.spans.into_iter() {
            // Create the preceding plain text span.
            let range = last_position..span.range.start;
            if !range.is_empty() {
                spans.push(StyledIndexedSpan::new_range(range, plain_style));
            }
            last_position = span.range.end;
            // Add the styled span
            spans.push(span);
        }

        if last_position < source.len() {
            spans.push(StyledIndexedSpan::new_range(
                last_position..source.len(),
                plain_style,
            ));
        }

        let spans = spans
            .into_iter()
            .map(|span| span.into_cursive(source.as_str()))
            .collect();
        CursiveStyledString::with_spans(source, spans)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StyledIndexedSpan {
    pub attr: Style,
    // [start, end)
    pub range: Range<usize>,
}
impl StyledIndexedSpan {
    pub fn new(source: &str, attr: Style) -> Self {
        Self::new_range(0..source.len(), attr)
    }

    pub fn new_range(range: Range<usize>, attr: Style) -> Self {
        Self { range, attr }
    }

    pub fn resolve<'a>(&self, source: &'a str) -> &'a str {
        &source[self.range.clone()]
    }

    /// The length of the span, though note that it doesn't verify it.
    pub fn len(&self) -> usize {
        self.range.len()
    }

    pub fn is_empty(&self) -> bool {
        self.range.is_empty()
    }

    pub fn offset(&mut self, offset: usize) {
        self.range.end += offset;
        self.range.start += offset;
    }

    pub fn contains(&self, idx: usize) -> bool {
        self.range.contains(&idx)
    }

    pub fn into_cursive(self, source: &str) -> CursiveStyledIndexedSpan {
        let content = self.resolve(source);
        // This creates it with a span of [0, content.len()), which is incorrect as it needs to be
        // at [self.start, self.end).
        // We are using this constructor rather than manually constructing so that I don't have to
        // bother adding the unicode width library cursive uses as a direct dependency.
        let mut span = CursiveStyledIndexedSpan::simple_borrowed(content, self.attr);
        span.content.offset(self.range.start);
        assert_eq!(span.resolve(source).content, content);
        span
    }

    pub fn intersection(&self, range: Range<usize>) -> Option<Range<usize>> {
        range_intersection(self.range.clone(), range)
    }

    pub fn split_at(&self, idx: usize) -> (Option<StyledIndexedSpan>, Option<StyledIndexedSpan>) {
        if self.contains(idx) {
            let left_span = self.range.start..idx;
            let left_span = if left_span.is_empty() {
                None
            } else {
                Some(StyledIndexedSpan::new_range(left_span, self.attr))
            };
            let right_span = idx..self.range.end;
            let right_span = if right_span.is_empty() {
                None
            } else {
                Some(StyledIndexedSpan::new_range(right_span, self.attr))
            };

            (left_span, right_span)
        } else {
            (None, None)
        }
    }
}

// r1 intersected with r2
fn range_intersection(r1: Range<usize>, r2: Range<usize>) -> Option<Range<usize>> {
    if r1.is_empty() || r2.is_empty() || r1.start >= r2.end || r2.start >= r1.end {
        None
    } else {
        // Since we've already filtered out the empty case and the case where it is outside
        // we can just choose the values.
        Some(r1.start.max(r2.start)..r1.end.min(r2.end))
    }
}

// TODO: this could probably be simplified
/// Remove re from r1
/// Returns two ranges as the re may be in the middle of it
fn range_remove(
    r1: Range<usize>,
    re: Range<usize>,
) -> (Option<Range<usize>>, Option<Range<usize>>) {
    if re.is_empty() && !r1.is_empty() {
        // the intersection is empty, and r1 is not empty
        return (Some(r1), None);
    } else if re.is_empty() || r1.is_empty() || r1 == re {
        // we make so if r1 is empty it returns None anyway.
        return (None, None);
    }

    // Get the intersection because then we only have to reason about the part inside it
    let re = if let Some(re) = range_intersection(r1.clone(), re) {
        re
    } else {
        return (Some(r1), None);
    };

    if re.start > r1.start {
        // So the start is past the originator's start.
        // This means we have a left remaining, and maybe a right remainng
        let left = r1.start..re.start;
        if re.end >= r1.end {
            // There is no right remaining portion
            (Some(left), None)
        } else {
            // There is a right remaining portion along with the start portion
            let right = re.end..r1.end;
            (Some(left), Some(right))
        }
    } else {
        // There is no left hand side.
        if re.end >= r1.end {
            // There no right side
            (None, None)
        } else {
            let right = re.end..r1.end;
            (None, Some(right))
        }
    }
}

fn range_add(r1: Range<usize>, amount: usize) -> Range<usize> {
    (r1.start + amount)..(r1.end + amount)
}

fn range_subtract(r1: Range<usize>, amount: usize) -> Range<usize> {
    (r1.start - amount)..(r1.end - amount)
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use cursive::theme::{Color, ColorStyle, ColorType, Effect, Style};

    use super::{range_intersection, range_remove, StyledIndexedSpan, StyledString};

    #[test]
    #[allow(clippy::clippy::reversed_empty_ranges)]
    fn test_range_intersection() {
        // Emptiness
        assert_eq!(range_intersection(0..0, 0..0), None);
        assert_eq!(range_intersection(0..0, 1..5), None);
        assert_eq!(range_intersection(1..1, 0..5), None);
        assert_eq!(range_intersection(6..44, 44..6), None);
        assert_eq!(
            range_intersection(usize::MAX..usize::MAX, 0..usize::MAX),
            None
        );
        // Simple equivalency
        assert_eq!(range_intersection(0..1, 0..1), Some(0..1));
        assert_eq!(range_intersection(5..9, 5..9), Some(5..9));
        assert_eq!(
            range_intersection(9999..10000, 9999..10000),
            Some(9999..10000)
        );
        assert_eq!(
            range_intersection(0..usize::MAX, 0..usize::MAX),
            Some(0..usize::MAX)
        );

        //  Sub slices
        assert_eq!(range_intersection(0..10, 0..5), Some(0..5));
        assert_eq!(range_intersection(0..5, 0..10), Some(0..5));
        assert_eq!(range_intersection(0..2, 0..1), Some(0..1));
        assert_eq!(range_intersection(0..1, 0..2), Some(0..1));
        assert_eq!(range_intersection(0..50, 0..49), Some(0..49));
        assert_eq!(range_intersection(5..10, 4..8), Some(5..8));
        assert_eq!(range_intersection(59..90, 20..52), None);
        assert_eq!(range_intersection(59..90, 20..59), None);
        assert_eq!(range_intersection(59..90, 20..60), Some(59..60));
    }

    #[test]
    #[allow(clippy::clippy::reversed_empty_ranges)]
    fn test_range_removal() {
        let empty: (Option<Range<usize>>, Option<Range<usize>>) = (None, None);
        // Empty
        assert_eq!(range_remove(0..0, 0..0), empty);
        assert_eq!(range_remove(0..0, 1..0), empty);
        assert_eq!(range_remove(0..5, 0..5), empty);
        assert_eq!(range_remove(0..1, 0..1), empty);
        assert_eq!(range_remove(0..usize::MAX, 0..usize::MAX), empty);
        // Left Keep
        assert_eq!(range_remove(0..5, 4..5), (Some(0..4), None));
        assert_eq!(range_remove(0..5, 3..5), (Some(0..3), None));
        assert_eq!(range_remove(0..5, 2..5), (Some(0..2), None));
        assert_eq!(range_remove(0..5, 1..5), (Some(0..1), None));
        // Miss
        assert_eq!(range_remove(9..14, 0..9), (Some(9..14), None));
        assert_eq!(range_remove(9..14, 0..3), (Some(9..14), None));
        assert_eq!(range_remove(9..14, 14..52), (Some(9..14), None));
        // Right Keep
        assert_eq!(range_remove(0..5, 0..4), (None, Some(4..5)));
        assert_eq!(range_remove(0..5, 0..3), (None, Some(3..5)));
        assert_eq!(range_remove(0..5, 0..2), (None, Some(2..5)));
        assert_eq!(range_remove(0..5, 0..1), (None, Some(1..5)));
        // Both Keep
        assert_eq!(range_remove(0..10, 1..9), (Some(0..1), Some(9..10)));
        assert_eq!(range_remove(0..10, 1..8), (Some(0..1), Some(8..10)));
        assert_eq!(range_remove(0..10, 1..2), (Some(0..1), Some(2..10)));
        assert_eq!(range_remove(0..10, 6..9), (Some(0..6), Some(9..10)));
    }

    #[test]
    fn test_add_span_intersect() {
        let simple_effect = Effect::Underline;
        let simple_style = simple_effect.into();
        let style2_color = ColorStyle::new(
            ColorType::Color(Color::Rgb(0xFF, 0xAA, 0xFF)),
            ColorType::Color(Color::Rgb(0x00, 0xF0, 0x0F)),
        );
        let style2 = style2_color.into();
        let testing_span = StyledIndexedSpan::new_range(0..4, simple_style);
        let mut testing = StyledString {
            source: "Testing".to_owned(), // 0..7
            spans: vec![testing_span.clone()],
        };
        let original_testing = testing.clone();
        #[allow(clippy::clippy::eq_op)]
        {
            assert_eq!(testing, testing);
        }
        // Inserting empty
        testing.add_span_intersect(StyledIndexedSpan::new_range(0..0, style2));
        assert_eq!(testing, original_testing);

        // Inserting adjacent, but not intersecting
        let adjacent_span = StyledIndexedSpan::new_range(4..7, style2);
        testing.add_span_intersect(adjacent_span.clone());
        assert_eq!(testing.spans, &[testing_span, adjacent_span]);

        // For simplicities sake
        testing = original_testing.clone();

        let intersecting_span = StyledIndexedSpan::new_range(2..6, style2);
        testing.add_span_intersect(intersecting_span);
        assert_eq!(
            testing.spans,
            &[
                StyledIndexedSpan::new_range(0..2, simple_style),
                StyledIndexedSpan::new_range(2..4, Style::merge(&[simple_style, style2])),
                StyledIndexedSpan::new_range(4..6, style2),
            ]
        );

        let style3 = Style::merge(&[Effect::Italic.into(), Effect::Bold.into()]);
        let intersecting_span2 = StyledIndexedSpan::new_range(3..7, style3);
        testing.add_span_intersect(intersecting_span2);
        assert_eq!(
            testing.spans,
            &[
                StyledIndexedSpan::new_range(0..2, simple_style),
                StyledIndexedSpan::new_range(2..3, Style::merge(&[simple_style, style2])),
                StyledIndexedSpan::new_range(3..4, Style::merge(&[simple_style, style2, style3])),
                StyledIndexedSpan::new_range(4..6, Style::merge(&[style2, style3])),
                StyledIndexedSpan::new_range(6..7, style3)
            ]
        );
    }

    fn test_map_styles() {
        let mut text: StyledString = "Testing".into();
        assert_eq!(text.map_styles("te", "te"), &[]);
        assert_eq!(text.map_styles("", ""), &[]);
        assert_eq!(text.map_styles("al", "omega"), &[]);

        // "Test"
        let first_span = StyledIndexedSpan::new_range(0..4, Effect::Underline.into());
        text.spans.push(first_span.clone());
        // Equivalency checks
        assert_eq!(text.map_styles("te", "te"), &[first_span.clone()]);
        assert_eq!(text.map_styles("", ""), &[first_span.clone()]);
        assert_eq!(text.map_styles("te", "lo"), &[first_span.clone()]);
        // Outside bound
        assert_eq!(text.map_styles("ing", "asd"), &[first_span.clone()]);
        assert_eq!(text.map_styles("ing", "asdft"), &[first_span.clone()]);
        // Decrease
        assert_eq!(
            text.map_styles("te", "t"),
            &[StyledIndexedSpan::new_range(0..3, Effect::Underline.into())]
        );
    }

    // #[test]
    // fn test_replace() {
    //     let empty = StyledString::default();
    //     assert_eq!(empty.replace("a", "b").source(), "");
    //     // The internal structure is different so this could fail.
    //     // Wonderful eq implementation..
    //     assert_eq!(empty.replace("", "").source(), "");
    //     assert_eq!(empty.replace("", "b").source(), "b");
    //     assert_eq!(empty.replace("a", "").source(), "");
    //     let simple = StyledString::from("foo1bar1".to_owned());
    //     // empty
    //     assert_eq!(simple.replace("", "").source(), "foo1bar1");
    //     // alternating
    //     assert_eq!(simple.replace("", "z").source(), "zfzozoz1zbzazrz1z");
    //     // nonexistant
    //     assert_eq!(simple.replace("z", "").source(), "foo1bar1");
    //     // identity
    //     assert_eq!(simple.replace("f", "f").source(), "foo1bar1");
    //     assert_eq!(simple.replace("foo1bar1", "foo1bar1").source(), "foo1bar1");

    //     assert_eq!(simple.replace("f", "a").source(), "aoo1bar1");
    //     assert_eq!(simple.replace("foo", "alpha").source(), "alpha1bar1");
    // }
}
