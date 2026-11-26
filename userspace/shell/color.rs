pub(super) const PROMPT: &[u8] = b"\x1b[32mclsh> \x1b[0m";

pub(super) const RED: &[u8] = b"\x1b[31m";
pub(super) const GREEN: &[u8] = b"\x1b[32m";
pub(super) const YELLOW: &[u8] = b"\x1b[33m";

pub(super) fn print_line(color: &[u8], message: &[u8]) {
    super::print_line(&[color, message, b"\x1b[0m"]);
}
