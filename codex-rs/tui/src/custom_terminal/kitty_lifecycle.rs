use std::io;
use std::io::Write;

use crossterm::execute;
use crossterm::terminal::EnterAlternateScreen;
use crossterm::terminal::LeaveAlternateScreen;
use ratatui::backend::Backend;
use ratatui::backend::ClearType;
use ratatui::layout::Position;

use super::Terminal;
use crate::terminal_image::KittyImage;

impl<B> Terminal<B>
where
    B: Backend<Error = io::Error>,
    B: Write,
{
    /// Clear the alternate screen for a full transcript rebuild and force a full redraw.
    ///
    /// A main-screen caller must use [`Self::clear_scrollback_and_visible_screen_ansi`] instead so
    /// deleting image definitions cannot leave placeholder rows in retained scrollback unresolved.
    pub fn clear_visible_screen(&mut self) -> io::Result<()> {
        debug_assert!(self.kitty_image_registries.alternate_screen_active());
        let home = Position { x: 0, y: 0 };
        self.delete_active_kitty_image_definitions()?;
        // Some terminals (notably Terminal.app) behave more reliably if we pair ED2
        // with an explicit cursor-home before/after, matching the common `clear`
        // sequence (`CSI 2J` + `CSI H`).
        self.set_cursor_position(home)?;
        self.backend.clear_region(ClearType::All)?;
        self.set_cursor_position(home)?;
        std::io::Write::flush(&mut self.backend)?;
        self.visible_history_rows = 0;
        self.previous_buffer_mut().reset();
        Ok(())
    }

    /// Hard-reset scrollback + visible screen using an explicit ANSI sequence.
    ///
    /// Some terminals behave more reliably when purge + clear are emitted as a
    /// single ANSI sequence instead of separate backend commands.
    pub fn clear_scrollback_and_visible_screen_ansi(&mut self) -> io::Result<()> {
        self.delete_active_kitty_image_definitions()?;
        // Reset scroll region + style state, home cursor, clear screen, purge scrollback.
        // The order matches the common shell `clear && printf '\\e[3J'` behavior.
        write!(self.backend, "\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H")?;
        std::io::Write::flush(&mut self.backend)?;
        self.last_known_cursor_pos = Position { x: 0, y: 0 };
        self.visible_history_rows = 0;
        self.previous_buffer_mut().reset();
        Ok(())
    }

    pub(crate) fn write_kitty_image_definitions<'a>(
        &mut self,
        images: impl IntoIterator<Item = &'a KittyImage>,
    ) -> io::Result<()> {
        self.kitty_image_registries
            .write_definitions(&mut self.backend, images)
    }

    fn delete_active_kitty_image_definitions(&mut self) -> io::Result<()> {
        self.kitty_image_registries.delete_active(&mut self.backend)
    }

    pub(crate) fn enter_alternate_screen(&mut self) -> io::Result<()> {
        if self.kitty_image_registries.alternate_screen_active() {
            return Ok(());
        }
        execute!(self.backend, EnterAlternateScreen)?;
        self.kitty_image_registries.activate_alternate_screen();
        Ok(())
    }

    pub(crate) fn resume_alternate_screen(&mut self) -> io::Result<()> {
        debug_assert!(self.kitty_image_registries.alternate_screen_active());
        // DECSET 1049 clears the alternate buffer's text and graphics. Forget the terminal-side
        // definitions before re-entering so subsequent history insertion retransmits its images.
        self.kitty_image_registries.forget_alternate_screen();
        execute!(self.backend, EnterAlternateScreen)?;
        Ok(())
    }

    pub(crate) fn leave_alternate_screen(&mut self) -> io::Result<()> {
        if !self.kitty_image_registries.alternate_screen_active() {
            return Ok(());
        }
        self.delete_active_kitty_image_definitions()?;
        execute!(self.backend, LeaveAlternateScreen)?;
        self.kitty_image_registries.activate_main_screen();
        Ok(())
    }
}
