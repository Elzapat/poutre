pub(crate) const VOXEL_SIZE: f32 = 0.1;
pub(crate) const CHUNK_SIZE: u32 = 32;
pub(crate) const WORLD_CHUNKS: u32 = 600;
pub(crate) const WORLD_SIZE: f32 = VOXEL_SIZE * CHUNK_SIZE as f32 * WORLD_CHUNKS as f32;
pub(crate) const RENDER_DISTANCE_CHUNKS: u32 = 160;

const MAX_PLAYER_HEIGHT: f32 = 100.0;
const MAX_PITCH: f32 = 1.55;

pub(crate) fn validate_transform(
    x: f32,
    y: f32,
    z: f32,
    yaw: f32,
    pitch: f32,
) -> Result<(), String> {
    if ![x, y, z, yaw, pitch].into_iter().all(f32::is_finite) {
        return Err("transform values must be finite".into());
    }
    if !(0.0..=WORLD_SIZE).contains(&x) || !(0.0..=WORLD_SIZE).contains(&z) {
        return Err("player position is outside the world".into());
    }
    if !(0.0..=MAX_PLAYER_HEIGHT).contains(&y) {
        return Err("player height is outside the world".into());
    }
    if !(-MAX_PITCH..=MAX_PITCH).contains(&pitch) {
        return Err("player pitch is outside the allowed range".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_world_transform() {
        assert!(validate_transform(960.0, 25.0, 960.0, 1.0, 0.5).is_ok());
    }

    #[test]
    fn rejects_non_finite_transform() {
        assert_eq!(
            validate_transform(f32::NAN, 25.0, 960.0, 0.0, 0.0),
            Err("transform values must be finite".into())
        );
    }

    #[test]
    fn rejects_transform_outside_world() {
        assert_eq!(
            validate_transform(WORLD_SIZE + 0.1, 25.0, 960.0, 0.0, 0.0),
            Err("player position is outside the world".into())
        );
        assert_eq!(
            validate_transform(960.0, 25.0, 960.0, 0.0, MAX_PITCH + 0.1),
            Err("player pitch is outside the allowed range".into())
        );
    }
}
