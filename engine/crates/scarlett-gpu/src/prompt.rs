//! A question the runtime puts to the player in a box in the middle of the
//! title's picture (the account picker): the system-call side
//! (`scarlett-hle`) says what to show, the presenter draws it over each frame
//! (`host::panel::Prompt`). Characters the box's font lacks are shown as their
//! plain capitals (accents dropped).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// What is shown: a number that changes with the text, and its lines.
static TEXT: Mutex<Option<(u64, Vec<String>)>> = Mutex::new(None);
static NEXT: AtomicU64 = AtomicU64::new(1);
/// When the presenter last drew a frame.
static PRESENTED: Mutex<Option<Instant>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Shows `lines` over every frame from the next one on, until [`hide`].
pub fn show(lines: &[&str]) {
    let lines = lines.iter().map(|l| fold(l)).collect();
    *lock(&TEXT) = Some((NEXT.fetch_add(1, Ordering::Relaxed), lines));
}

pub fn hide() {
    *lock(&TEXT) = None;
}

/// Whether frames are being presented (one in the last two seconds): only
/// then does a prompt reach the player.
pub fn presenting() -> bool {
    lock(&PRESENTED).is_some_and(|at| at.elapsed() < Duration::from_secs(2))
}

pub(crate) fn current() -> Option<(u64, Vec<String>)> {
    lock(&TEXT).clone()
}

pub(crate) fn presented() {
    *lock(&PRESENTED) = Some(Instant::now());
}

/// `text` in the box's font: capitals, accented letters as their plain ones.
pub fn fold(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'Á' | 'À' | 'Â' | 'Ã' | 'Ä' => 'A',
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'E',
            'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => 'I',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' => 'O',
            'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ù' | 'Û' | 'Ü' => 'U',
            'ç' | 'Ç' => 'C',
            'ñ' | 'Ñ' => 'N',
            c if c.is_ascii() => c.to_ascii_uppercase(),
            _ => '?',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_folded_to_the_boxs_font() {
        assert_eq!(fold("Seleção de conta: João"), "SELECAO DE CONTA: JOAO");
        assert_eq!(fold("名"), "?");
    }

    #[test]
    fn shown_text_is_numbered_and_hidden() {
        show(&["a"]);
        let (first, lines) = current().unwrap();
        assert_eq!(lines, ["A"]);
        show(&["b"]);
        assert_ne!(current().unwrap().0, first, "new text, new number");
        hide();
        assert!(current().is_none());
    }
}
