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
    commit: "793ebe976cd99bbb3899ae6a8c8cac4ad0b7b50f",
};

pub const ALL_PINS: &[DecompPin] = &[POKEPLATINUM_PIN, POKEHEARTGOLD_PIN];
