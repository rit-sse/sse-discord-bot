use crate::{Data, Error};

pub mod age;
pub mod verify;

pub fn all() -> Vec<poise::Command<Data, Error>> {
    vec![age::age(), verify::verify()]
}
