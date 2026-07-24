pub use boot_contracts::generation::{
    Component, DecodeError, Generation, Grant, Object, StateBinding, generation_identity,
};

use boot_contracts::generation::{KIND_BOOTSTRAP, KIND_COMPONENT};

pub fn decode(bytes: &[u8]) -> Result<Generation<'_>, DecodeError> {
    let generation = Generation::decode(bytes)?;
    for index in 0..generation.object_count() {
        let object = generation.object(index)?;
        if matches!(object.kind, KIND_BOOTSTRAP | KIND_COMPONENT) {
            crate::component::decode(object.bytes).map_err(|_| DecodeError::BadBounds)?;
        }
    }
    Ok(generation)
}
