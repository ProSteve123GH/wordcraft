//! Android's soft keyboard delivers its edits through a *buffer*, not key events: GameTextInput's
//! `GameTextInputState` holds the text an IME is working on, with a composing region for the span
//! it is editing. The document never sees Backspace or `Key` events from it; it sees the buffer.
//!
//! The buffer is not the document. A word is typed as a *composition* — the IME fills a span, its
//! candidate strip rewrites it repeatedly, and a commit finally makes it real — so the buffer must
//! be diffed against what the document already has, not appended to. While a composition is live
//! the document keeps its own text behind it: the composition is an overlay (autocorrect can still
//! rewrite the word underneath it later).
//!
//! [`ImeBuffer::poll`] turns one read of the buffer into a preedit update and/or a document edit
//! ([`ImeUpdate`]), with that translation in one place, independent of Android and egui, so it can
//! be unit-tested on any host. The host shows a preedit under the caret and turns [`ImeUpdate`]
//! into egui's IME events.
//!
//! Only compositions that reach the end of the buffer are supported — the normal case: the IME
//! composes at the caret. The host resets the buffer whenever the caret moves out from under
//! typing (a page tap, a dialog), so the buffer's text always sits immediately before the caret
//! in the document.

/// One read of the IME buffer, as the document and its preedit should see it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImeUpdate {
    /// Change the preedit shown under the caret to this (`Some("")` clears it). Absent when it
    /// should stay exactly as it is.
    pub preedit: Option<String>,
    /// Apply to the document: delete this many characters immediately before the caret, then
    /// insert this string after them. `None` leaves the document alone.
    pub edit: Option<(usize, String)>,
}

/// The state of one open soft-keyboard session against the document, from opening to reset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImeBuffer {
    /// What the document has received from the buffer so far (no composition).
    synced: String,
    /// The composition text as currently shown under the caret, if there is one.
    preedit: String,
}

/// The byte offset of the character boundary at or before `target`: a composition region's start
/// rounds down onto the character it covers. A region arrives as integers Java counted over its
/// own UTF-16 text; byte and character boundaries agree on ASCII only, so the worst an offset can
/// do is shift the span a little — it never splits a character or panics.
fn floor_boundary_byte(text: &str, target: usize) -> usize {
    let mut start = 0;
    for (_, ch) in text.char_indices() {
        let end = start + ch.len_utf8();
        if end > target {
            return start;
        }
        start = end;
    }
    start
}

/// The byte offset of the character boundary at or after `target`: a composition region's end is
/// exclusive, so it rounds up onto the boundary just past what it covered (or the text's end).
fn ceil_boundary_byte(text: &str, target: usize) -> usize {
    let target = target.min(text.len());
    for (offset, ch) in text.char_indices() {
        let end = offset + ch.len_utf8();
        if end >= target {
            return end;
        }
    }
    text.len()
}

impl ImeBuffer {
    /// A fresh buffer: nothing committed, nothing composed.
    pub fn new() -> Self {
        Self::default()
    }

    /// The composition text currently shown under the caret, if there is one.
    pub fn preedit(&self) -> &str {
        &self.preedit
    }

    /// Start typing from a different place. The host empties the IME buffer as well (after a page
    /// tap or when the keyboard leaves the page), so this drops any composition: the buffer's
    /// text always means "typed at the current caret".
    pub fn reset(&mut self) {
        self.synced.clear();
        self.preedit.clear();
    }

    /// Bring the buffer into the document. `text` is the game's text input buffer and `compose`
    /// its composing region as `(start, end)` byte offsets; `None` (or a reversed or empty
    /// region, which a junk report can be) means nothing is composing.
    pub fn poll(&mut self, text: &str, compose: Option<(usize, usize)>) -> ImeUpdate {
        if let Some((start, end)) = compose
            && end > start
        {
            let begin = floor_boundary_byte(text, start);
            let finish = ceil_boundary_byte(text, end.max(begin));
            if finish >= text.len() {
                // A live composition at the caret: the buffer's region is an overlay, and the
                // document keeps what it already has. When it commits, the whole buffer is
                // diffed against `synced`, which covers a rewrite of the word under the
                // composition as well.
                let preedit = text[begin..finish].to_owned();
                if preedit == self.preedit {
                    return ImeUpdate::default();
                }
                self.preedit = preedit.clone();
                return ImeUpdate { preedit: Some(preedit), edit: None };
            }

            // A composition with text after it — Gboard rewriting an earlier word while typing
            // went on. Without mirroring the whole document into the IME, the safe move is to
            // treat the buffer as committed text now and diff it against the document.
        }

        // No composition (or none left): the buffer is real text. Against `synced` it is one
        // edit — typed ahead, Backspace in the strip, a whole-word autocorrect as one rewrite.
        let edit = diff_edit(&self.synced, text);
        self.synced.clear();
        self.synced.push_str(text);
        let had_preedit = !self.preedit.is_empty();
        self.preedit.clear();
        ImeUpdate { preedit: had_preedit.then(|| String::new()), edit }
    }
}

/// One document edit that turns `old` into `new`: the longest shared prefix and the longest shared
/// suffix stay, the characters between them change. Shared spans never overlap, so the edit is
/// minimal whatever the rewrite (a fast append, a Backspace, an autocorrect).
fn diff_edit(old: &str, new: &str) -> Option<(usize, String)> {
    // Work in chars: the document engine counts them the way the IME's spans intend to.
    let old: Vec<char> = old.chars().collect();
    let new: Vec<char> = new.chars().collect();
    let shared = old.len().min(new.len());
    let mut prefix = 0;
    while prefix < shared && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < shared - prefix && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix] {
        suffix += 1;
    }
    let delete_chars = old.len() - prefix - suffix;
    let insert: String = new[prefix..new.len() - suffix].iter().collect();
    (delete_chars > 0 || !insert.is_empty()).then_some((delete_chars, insert))
}

#[cfg(test)]
mod tests {
    use super::{ImeBuffer, ImeUpdate};

    /// A poll's edit, for matching.
    fn edit(out: ImeUpdate) -> Option<(usize, String)> {
        out.edit
    }

    #[test]
    fn empty_buffer_polls_to_nothing() {
        let mut b = ImeBuffer::new();
        assert_eq!(b.poll("", None), ImeUpdate::default());
        // A zero-length or reversed region is no composition at all.
        assert_eq!(b.poll("", Some((0, 0))), ImeUpdate::default());
    }

    #[test]
    fn types_a_word_letter_by_letter() {
        let mut b = ImeBuffer::new();
        assert_eq!(edit(b.poll("m", None)), Some((0, "m".into())));
        assert_eq!(edit(b.poll("mi", None)), Some((0, "i".into())));
        assert_eq!(edit(b.poll("mnc", None)), Some((0, "c".into())));
    }

    #[test]
    fn a_frame_that_misses_several_keystrokes_commits_them_all() {
        // The app's frame rate can lag behind the thumbs: one read carries every keystroke
        // since the last one, so "minecraft" is never "mncrft" behind the app's back.
        let mut b = ImeBuffer::new();
        assert_eq!(edit(b.poll("a", None)), Some((0, "a".into())));
        assert_eq!(edit(b.poll("minecraft", None)), Some((0, "inecraft".into())));
    }

    #[test]
    fn composition_is_an_overlay_that_commits_once() {
        let mut b = ImeBuffer::new();
        // The IME composes "min": a preedit, and nothing in the document.
        assert_eq!(b.poll("min", Some((0, 3))), ImeUpdate { preedit: Some("min".into()), edit: None });
        assert_eq!(b.preedit(), "min");
        // The candidate strip rewrites the composition: another preedit, still nothing.
        assert_eq!(b.poll("mint", Some((0, 4))), ImeUpdate { preedit: Some("mint".into()), edit: None });
        // The same composition again lands as nothing.
        assert_eq!(b.poll("mint", Some((0, 4))), ImeUpdate::default());
        // The user picked a candidate: the composition is gone and the buffer is text.
        assert_eq!(b.preedit(), "mint");
        let out = b.poll("Mint", None);
        assert_eq!(out.preedit, Some(String::new()), "the preedit clears");
        assert_eq!(out.edit, Some((0, "Mint".into())));
        // Continue typing after the commit.
        assert_eq!(edit(b.poll("Minecraft", None)), Some((0, "ecraft".into())));
    }

    #[test]
    fn a_commit_its_region_skipped_still_arrives() {
        // A commit (compose None) that follows another commit and a fast thumb: whatever is in
        // the buffer beyond `synced` is typed now, not before.
        let mut b = ImeBuffer::new();
        assert_eq!(edit(b.poll("mi", None)), Some((0, "mi".into())));
        // "minecraft" replaced "mi" wholesale (a retro rewrite): keep the shared start.
        assert_eq!(edit(b.poll("minecraft", None)), Some((0, "inecraft".into())));
    }

    #[test]
    fn composition_over_committed_text_replaces_it_on_commit() {
        // "teh " was committed (the word is in the document); Gboard then composes over an
        // earlier word while typing went on (a retro rewrite, text after the region): the
        // rewrite lands in the document as one diff.
        let mut b = ImeBuffer::new();
        assert_eq!(edit(b.poll("teh ", None)), Some((0, "teh ".into())));
        let out = b.poll("the teh ", Some((4, 7)));
        assert_eq!(out.preedit, None, "a region not reaching the buffer end is not an overlay");
        assert_eq!(out.edit, Some((2, "he the".into())), "\"the the\" from \"teh \" at the caret");
        // The buffer has settled; the next poll is a no-op.
        assert_eq!(b.poll("the teh ", Some((4, 8))), ImeUpdate::default());
    }

    #[test]
    fn a_composition_after_committed_text_still_overlays() {
        // A region that reaches the end of the buffer is the normal live composition, however
        // much text came before it in the buffer.
        let mut b = ImeBuffer::new();
        assert_eq!(edit(b.poll("AB ", None)), Some((0, "AB ".into())));
        let out = b.poll("AB de", Some((3, 5)));
        assert_eq!(out.preedit, Some("de".into()));
        assert_eq!(out.edit, None, "the document keeps its own text while composing");
        let out = b.poll("AB de", None);
        assert_eq!(out.preedit, Some(String::new()));
        assert_eq!(out.edit, Some((0, "de".into())));
    }

    #[test]
    fn composition_inside_the_buffer() {
        // The composition may sit after committed text, not at the buffer's start.
        let mut b = ImeBuffer::new();
        assert_eq!(edit(b.poll("AB", None)), Some((0, "AB".into())));
        // Both offsets are 2 ("ABde"): the byte offset of 'd' and its char index agree in ASCII.
        assert_eq!(b.poll("ABde", Some((2, 4))).preedit, Some("de".into()));
        // Commit: the document gets only what it didn't have yet.
        let out = b.poll("ABde", None);
        assert_eq!(out.edit, Some((0, "de".into())));
        assert_eq!(out.preedit, Some(String::new()), "the preedit clears");
    }

    #[test]
    fn backspace_shrinks_the_buffer() {
        let mut b = ImeBuffer::new();
        assert_eq!(edit(b.poll("ab", None)), Some((0, "ab".into())));
        assert_eq!(edit(b.poll("a", None)), Some((1, String::new())));
        // A backspace that compounds with typing in the same read.
        assert_eq!(edit(b.poll("ac", None)), Some((1, "c".into())));
    }

    #[test]
    fn reset_forgets_the_composition_and_the_sync() {
        let mut b = ImeBuffer::new();
        assert_eq!(b.poll("ab", Some((0, 2))), ImeUpdate { preedit: Some("ab".into()), edit: None });
        b.reset();
        assert!(b.preedit().is_empty());
        // Typing from a new place commits a fresh word, not the old one again.
        assert_eq!(edit(b.poll("cd", None)), Some((0, "cd".into())));
    }

    #[test]
    fn unicode_compositions_stay_on_char_boundaries() {
        // Offsets that arrive mid-character or past the end round onto a boundary, not a panic.
        let mut b = ImeBuffer::new();
        assert_eq!(b.poll("ö", Some((0, 2))).preedit, Some("ö".into()));
        assert_eq!(edit(b.poll("öa", None)), Some((0, "öa".into())));
        // Multibyte edits are still counted in chars: a region past the end rounds to the text.
        b.reset();
        assert_eq!(b.poll("abö", Some((2, 6))).preedit, Some("ö".into()), "the region ends where the text does");
        let out = b.poll("abö", None);
        assert_eq!(out.edit, Some((0, "abö".into())));
    }

    #[test]
    fn a_cleared_composition_commits_nothing() {
        // The user backs out of a composition with an empty buffer: the ghost clears, the
        // document (already empty) gets no edit.
        let mut b = ImeBuffer::new();
        assert_eq!(b.poll("min", Some((0, 3))), ImeUpdate { preedit: Some("min".into()), edit: None });
        let out = b.poll("", None);
        assert_eq!(out.preedit, Some(String::new()));
        assert_eq!(out.edit, None, "an empty diff against an empty start is no edit");
        assert!(b.preedit().is_empty());
    }
}
