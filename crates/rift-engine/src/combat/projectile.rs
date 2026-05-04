use glam::Vec3;

/// Type of projectile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectileKind {
    Arrow,
    // Future: FireArrow, PoisonArrow, etc.
}

/// A projectile entity component.
#[derive(Clone, Debug)]
pub struct Projectile {
    pub kind: ProjectileKind,
    pub position: Vec3,
    pub direction: Vec3,
    pub speed: f32,
    pub damage: f32,
    /// Remaining lifetime in seconds.
    pub lifetime: f32,
    /// How many targets this can pierce through (0 = dies on first hit).
    pub pierce_remaining: u32,
    /// Owner entity (to avoid self-hits).
    pub owner_is_player: bool,
    /// Visual size (for rendering the arrow mesh).
    pub size: f32,
}

impl Projectile {
    pub fn arrow(position: Vec3, direction: Vec3, damage: f32) -> Self {
        Self {
            kind: ProjectileKind::Arrow,
            position,
            direction: direction.normalize(),
            speed: 20.0,
            damage,
            lifetime: 2.0,
            pierce_remaining: 0,
            owner_is_player: true,
            size: 0.6,
        }
    }

    pub fn alive(&self) -> bool {
        self.lifetime > 0.0
    }

    pub fn tick(&mut self, dt: f32) {
        self.position += self.direction * self.speed * dt;
        self.lifetime -= dt;
    }
}

/// System: tick all projectiles, check collisions, apply damage.
pub fn projectile_system(
    projectiles: &mut Vec<Projectile>,
    enemies: &mut [(Vec3, f32, &mut f32)], // (position, radius, &mut health)
    dt: f32,
) -> Vec<ProjectileHit> {
    let mut hits = Vec::new();

    for proj in projectiles.iter_mut() {
        if !proj.alive() { continue; }
        proj.tick(dt);

        // Check collisions with enemies
        if proj.owner_is_player {
            for (i, (pos, radius, health)) in enemies.iter_mut().enumerate() {
                let dist = (proj.position - *pos).length();
                if dist < *radius + proj.size * 0.5 {
                    **health -= proj.damage;
                    hits.push(ProjectileHit {
                        position: proj.position,
                        target_index: i,
                        damage: proj.damage,
                    });

                    if proj.pierce_remaining > 0 {
                        proj.pierce_remaining -= 1;
                    } else {
                        proj.lifetime = 0.0; // Kill projectile
                        break;
                    }
                }
            }
        }
    }

    // Remove dead projectiles
    projectiles.retain(|p| p.alive());

    hits
}

/// Info about a projectile hitting a target.
#[derive(Clone, Debug)]
pub struct ProjectileHit {
    pub position: Vec3,
    pub target_index: usize,
    pub damage: f32,
}
