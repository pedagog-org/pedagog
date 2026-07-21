use core::fmt;
use std::fmt::Display;

#[derive(Clone)]
pub struct Command(pub String);

impl Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}