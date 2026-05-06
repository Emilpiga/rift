/// Rift progression state — tracks floor, progress, boss, and loot timer.
pub struct RiftState {
    pub floor: u32,
    pub progress: f32,
    pub progress_required: f32,
    pub boss_spawned: bool,
    pub boss_killed: bool,
    pub timer: f32,
    pub floor_complete: bool,
    pub loot_timer: f32,
}

impl RiftState {
    pub fn new(floor: u32) -> Self {
        let progress_required = 80.0 + floor as f32 * 20.0;
        Self {
            floor,
            progress: 0.0,
            progress_required,
            boss_spawned: false,
            boss_killed: false,
            timer: 0.0,
            floor_complete: false,
            loot_timer: 0.0,
        }
    }

    pub fn progress_percent(&self) -> f32 {
        (self.progress / self.progress_required * 100.0).min(100.0)
    }

    pub fn boss_speed(&self) -> f32 {
        3.0 + self.floor as f32 * 0.5
    }
}
