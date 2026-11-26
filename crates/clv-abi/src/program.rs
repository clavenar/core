macro_rules! programs {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        /// A stable selector for a built-in executable.
        #[derive(Clone, Copy, PartialEq, Eq)]
        #[repr(u8)]
        pub enum Program { $($variant),+ }

        pub const PROGRAMS: &[(Program, &str)] = &[$((Program::$variant, $name)),+];
    };
}

// Append entries to preserve the selectors used by existing binaries.
programs! {
    Hello => "hello",
    Echo => "echo",
    Grep => "grep",
}

impl Program {
    pub const fn from_raw(selector: usize) -> Option<Self> {
        if selector < PROGRAMS.len() {
            Some(PROGRAMS[selector].0)
        } else {
            None
        }
    }

    pub fn named(name: &[u8]) -> Option<Self> {
        PROGRAMS
            .iter()
            .find(|(_, entry)| entry.as_bytes() == name)
            .map(|&(program, _)| program)
    }

    pub const fn name(self) -> &'static str {
        PROGRAMS[self as usize].1
    }
}
