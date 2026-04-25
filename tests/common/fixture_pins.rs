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
    commit: "44a6337b419375e41ca8bbe42a9e16806d3c56b0",
};

pub const POKEHEARTGOLD_PIN: DecompPin = DecompPin {
    name: "pokeheartgold",
    repo_url: "https://github.com/pret/pokeheartgold.git",
    commit: "bdf1530b5f273ecb221756620a5e0043c7e2e15e",
};

pub const ALL_PINS: &[DecompPin] = &[POKEPLATINUM_PIN, POKEHEARTGOLD_PIN];
