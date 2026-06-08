//! Version-controlled commit pins for decomp fixture repositories.
//!
//! Bumping a pin is a conscious, version-controlled action.
//! Local drift does not accumulate.

pub struct DecompPin {
    pub name: &'static str,
    pub repo_url: &'static str,
    pub commit: &'static str,
}

pub const POKEPLATINUM_PIN: DecompPin = DecompPin {
    name: "pokeplatinum",
    repo_url: "https://github.com/pret/pokeplatinum.git",
    commit: "67a4b921678394e2c4881fbfda81e28ec53a92e1",
};

pub const POKEHEARTGOLD_PIN: DecompPin = DecompPin {
    name: "pokeheartgold",
    repo_url: "https://github.com/pret/pokeheartgold.git",
    commit: "79d73f74cc41e5615ff99b23588d416e96262fc0",
};

pub const ALL_PINS: &[DecompPin] = &[POKEPLATINUM_PIN, POKEHEARTGOLD_PIN];
