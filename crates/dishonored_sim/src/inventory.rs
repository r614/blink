//! Inventory and pickups (`DishonoredInventory`, `DisPickup_Base` and subclasses), decoded from
//! `Dishonored.exe` (research/dishonored/INVENTORY.md):
//!
//! - ammo: `AddAmmo` (0x00C052E0) adds `min(n, capacity - count)` and returns what it added;
//!   `RemoveAmmo` (0x00C05260) fails when fewer than `n` are left;
//! - elixirs: the pickup is refused at `m_nMax*Elixir` (0x00A22160), counts are clamped to
//!   `[0, max]` (0x00C04E10); drinking (0x00C04FC0) only works when health (mana) is below its
//!   maximum and adds `(int)((1 + Attribute_*PotionPotencyBonus) * m_n*ElixirValue)`, capped;
//! - stat pickups (food, ammo): each ammo amount is rolled once from `m_AmmoRanges`
//!   (`rand() % (max - min + 1) + min`, 0x00C3D4E0) and taking it moves what fits
//!   (0x00C05440); health/mana changes scale with the potion potency attributes (0x00A367A0);
//!   a stat pickup can be taken while one of its ammo types is not full or it heals/restores
//!   mana (0x00A36980);
//! - abstract items (`DisAbstractItemPickup`): `m_Quantity` of `m_pAbstractItem` (coins, runes,
//!   keys, notes).

use crate::events::SimEvent;
use crate::movement::{eye_position, view_vector};
use crate::powers::attribute;
use crate::state::{HeldItem, PickupState, PlayerId, PlayerState, WorldState};
use dishonored_ir::gameplay::GameplayConfig;
use dishonored_ir::items::{AmmoType, ElixirType, PickupKind, PlacedPickup};
use dishonored_world::{WorldQuery, channel};
use glam::Vec3;

pub const COINS_ITEM: &str = "Coins_AbsItm";
pub const RUNE_ITEM: &str = "Rune_AbsItm";

/// Count of an ammo type (bullets and bolts: the reserve plus the loaded round).
pub fn ammo(p: &PlayerState, t: AmmoType) -> u16 {
    match t {
        AmmoType::Bullet => p.left.pistol_reserve + p.left.pistol_loaded as u16,
        AmmoType::Bolt => p.left.bolts_reserve + p.left.bolts_loaded as u16,
        _ => p.inv.ammo[t as usize],
    }
}

/// `AddAmmo` (0x00C052E0): adds what fits under the capacity, returns the amount added.
pub fn add_ammo(p: &mut PlayerState, t: AmmoType, n: u16, cfg: &GameplayConfig) -> u16 {
    let cap = cfg.items.inventory.ammo_capacity[t as usize];
    let added = n.min(cap.saturating_sub(ammo(p, t)));
    match t {
        AmmoType::Bullet => p.left.pistol_reserve += added,
        AmmoType::Bolt => p.left.bolts_reserve += added,
        _ => p.inv.ammo[t as usize] += added,
    }
    added
}

/// `RemoveAmmo` (0x00C05260): all or nothing.
pub fn remove_ammo(p: &mut PlayerState, t: AmmoType, n: u16) -> bool {
    let slot = match t {
        AmmoType::Bullet => &mut p.left.pistol_reserve,
        AmmoType::Bolt => &mut p.left.bolts_reserve,
        _ => &mut p.inv.ammo[t as usize],
    };
    if *slot >= n {
        *slot -= n;
        true
    } else {
        false
    }
}

fn max_elixirs(cfg: &GameplayConfig, k: ElixirType) -> u8 {
    match k {
        ElixirType::Health => cfg.items.inventory.max_health_elixirs,
        ElixirType::Mana => cfg.items.inventory.max_mana_elixirs,
    }
}

/// `ModifyElixirCount` (0x00C04E10): clamped to `[0, max]`.
pub fn add_elixirs(p: &mut PlayerState, k: ElixirType, n: i32, cfg: &GameplayConfig) {
    let v = (p.inv.elixirs[k as usize] as i32 + n).clamp(0, max_elixirs(cfg, k) as i32);
    p.inv.elixirs[k as usize] = v as u8;
}

/// Drink an elixir (`Dis_ConsumeElixir` -> 0x00C04FC0). Returns false (nothing consumed) when
/// none is left or the stat is already full.
pub fn drink(p: &mut PlayerState, k: ElixirType, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) -> bool {
    if !p.alive || p.inv.elixirs[k as usize] == 0 {
        return false;
    }
    let inv = &cfg.items.inventory;
    match k {
        ElixirType::Health => {
            let max = crate::powers::health_max(p, cfg);
            if p.vitals.health >= max {
                return false;
            }
            let bonus = attribute(p, cfg, "Attribute_HealthPotionPotencyBonus", 0.0);
            let add = ((bonus + 1.0) * inv.health_elixir_value).trunc();
            p.vitals.health = (p.vitals.health + add).min(max);
        }
        ElixirType::Mana => {
            let max = crate::powers::mana_max(p, cfg);
            if p.vitals.mana >= max {
                return false;
            }
            let bonus = attribute(p, cfg, "Attribute_ManaPotionPotencyBonus", 0.0);
            let add = ((bonus + 1.0) * inv.mana_elixir_value).trunc();
            p.vitals.mana = (p.vitals.mana + add).min(max);
            p.vitals.mana_regen_cap = (p.vitals.mana + p.vitals.mana_regen_pool).min(max);
        }
    }
    p.inv.elixirs[k as usize] -= 1;
    events.push(SimEvent::ElixirUsed { id: p.id, mana: k == ElixirType::Mana });
    true
}

/// Deterministic stand-in for the original's `rand()` roll of an ammo range: the same pickup
/// in the same match always holds the same amount.
fn roll(seed: u32, pickup: u32, slot: u32, lo: i16, hi: i16) -> u16 {
    if hi <= lo {
        return lo.max(0) as u16;
    }
    let mut x = seed ^ pickup.wrapping_mul(0x9E37_79B1) ^ slot.wrapping_mul(0x85EB_CA77);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    let span = (hi - lo + 1) as u32;
    (lo as i32 + (x % span) as i32).max(0) as u16
}

/// Put a level's placed pickups into the world (once per map load). `stealable_owner` maps a
/// spawner name to the pawn it spawned, so carried pickups follow their NPC.
pub fn spawn_pickups(state: &mut WorldState, placed: &[PlacedPickup], seed: u32, stealable_owner: &dyn Fn(&str) -> Option<PlayerId>) {
    for pp in placed {
        let id = state.next_entity_id;
        state.next_entity_id = state.next_entity_id.wrapping_add(1);
        let mut ammo_left = [0u16; 8];
        if let PickupKind::Stat { ammo, .. } = &pp.kind {
            for (i, (lo, hi)) in ammo.iter().enumerate() {
                ammo_left[i] = roll(seed, id, i as u32, *lo, *hi);
            }
        }
        let carried_by = pp.stealable_from.as_deref().and_then(stealable_owner);
        state.pickups.push(PickupState { id, name: pp.name.clone(), package: pp.package.clone(), position: pp.position, kind: pp.kind.clone(), active: true, ammo_left, carried_by });
    }
}

pub fn item_count(state: &WorldState, owner: PlayerId, item: &str) -> i32 {
    state.abstract_items.iter().filter(|h| h.owner == owner && h.item == item).map(|h| h.quantity).sum()
}

/// `AddAbstractItem` / `RemoveAbstractItem` (`m_bRemoveAtZeroQuantity`).
pub fn give_item(state: &mut WorldState, owner: PlayerId, item: &str, quantity: i32) {
    if let Some(h) = state.abstract_items.iter_mut().find(|h| h.owner == owner && h.item == item) {
        h.quantity += quantity;
    } else if quantity > 0 {
        state.abstract_items.push(HeldItem { owner, item: item.to_string(), quantity });
    }
    state.abstract_items.retain(|h| h.quantity > 0);
}

/// Whether `p` could take this pickup now (`CanBePickedUp` of each class).
pub fn can_take(p: &PlayerState, pk: &PickupState, cfg: &GameplayConfig) -> bool {
    match &pk.kind {
        PickupKind::Elixir(k) => p.inv.elixirs[*k as usize] < max_elixirs(cfg, *k),
        PickupKind::Stat { health, mana, .. } => {
            *health != 0 || *mana != 0 || AmmoType::ALL.iter().any(|t| pk.ammo_left[*t as usize] > 0 && ammo(p, *t) < cfg.items.inventory.ammo_capacity[*t as usize])
        }
        PickupKind::Abstract { item, .. } => !item.is_empty(),
        PickupKind::BoneCharm(_) => true,
        PickupKind::Inventory { .. } => true,
    }
}

/// Take a pickup: apply what it gives. Returns true when it is used up (removed).
pub fn take(state: &mut WorldState, pi: usize, who: usize, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) -> bool {
    let pk = state.pickups[pi].clone();
    let pid = state.players[who].id;
    let mut used_up = true;
    match &pk.kind {
        PickupKind::Elixir(k) => add_elixirs(&mut state.players[who], *k, 1, cfg),
        PickupKind::Stat { health, mana, .. } => {
            let p = &mut state.players[who];
            if *health > 0 {
                let f = (attribute(p, cfg, "Attribute_HealthPotionPotencyBonus", 0.0) + 1.0) * *health as f32;
                p.vitals.health = (p.vitals.health + f.trunc()).min(crate::powers::health_max(p, cfg));
            }
            if *mana > 0 {
                let f = (attribute(p, cfg, "Attribute_ManaPotionPotencyBonus", 0.0) + 1.0) * *mana as f32;
                p.vitals.mana = (p.vitals.mana + f.trunc()).min(crate::powers::mana_max(p, cfg));
            }
            let mut left = 0;
            for t in AmmoType::ALL {
                let n = state.pickups[pi].ammo_left[t as usize];
                if n > 0 {
                    let added = add_ammo(&mut state.players[who], t, n, cfg);
                    state.pickups[pi].ammo_left[t as usize] -= added;
                    left += n - added;
                }
            }
            // Ammo that did not fit stays in the pickup (food is always consumed).
            used_up = left == 0 || *health != 0 || *mana != 0;
        }
        PickupKind::Abstract { item, quantity, .. } => {
            let short = item.rsplit('.').next().unwrap_or(item);
            let p = &mut state.players[who];
            if short == COINS_ITEM {
                p.inv.coins = p.inv.coins.saturating_add((*quantity).max(0) as u32);
            } else if short == RUNE_ITEM {
                p.inv.runes = p.inv.runes.saturating_add((*quantity).max(0) as u16);
            } else {
                give_item(state, pid, item, *quantity);
            }
        }
        PickupKind::BoneCharm(idx) => {
            let n = cfg.items.charms.len().min(32);
            let p = &mut state.players[who];
            // The original gives the charm placed in the level; untyped placements get the first
            // charm not owned yet, in list order from the pickup id (deterministic stand-in).
            let pick = idx.map(|i| i as usize).filter(|i| *i < n).or_else(|| (0..n).map(|k| (k + pk.id as usize) % n.max(1)).find(|i| p.inv.charms_owned & (1 << i) == 0));
            if let Some(i) = pick {
                p.inv.charms_owned |= 1 << i;
                // `m_bAutoEquipBoneCharm`: equipped while a slot is free.
                if (p.inv.charms_equipped.count_ones() as u8) < crate::powers::max_charms(p, cfg) {
                    p.inv.charms_equipped |= 1 << i;
                }
            }
        }
        PickupKind::Inventory { item } => give_item(state, pid, item, 1),
    }
    if used_up {
        state.pickups[pi].active = false;
        state.pickups[pi].carried_by = None;
    }
    events.push(SimEvent::PickedUp { id: pid, pickup: pk.id });
    used_up
}

/// The pickup `p` is looking at within `m_fMaxDistance` (closest to the view direction, visible).
pub fn pickup_in_view(state: &WorldState, p: &PlayerState, world: &dyn WorldQuery, cfg: &GameplayConfig) -> Option<usize> {
    let eye = eye_position(p, &cfg.movement);
    let dir = view_vector(p.yaw, p.pitch);
    let reach = cfg.items.inventory.interact_distance;
    let mut best: Option<(usize, f32)> = None;
    for (i, pk) in state.pickups.iter().enumerate() {
        if !pk.active {
            continue;
        }
        let pos = pickup_position(state, pk, &cfg.movement);
        let to = pos - eye;
        let d = to.length();
        if d > reach || d < 1e-3 {
            continue;
        }
        let cos = to.dot(dir) / d;
        // Highlight cone (LIKELY: the original traces the crosshair against the item's mesh).
        if cos < INTERACT_COS {
            continue;
        }
        if world.raycast(eye, to, d, channel::MELEE).is_some_and(|h| h.distance < d - 10.0) {
            continue;
        }
        if best.is_none_or(|(_, c)| cos > c) {
            best = Some((i, cos));
        }
    }
    best.map(|b| b.0)
}

/// `cos(30 deg)`: how far off the crosshair an item can be and still be used.
pub const INTERACT_COS: f32 = 0.866;

/// Where a pickup is: on its carrier's belt when an NPC carries it.
pub fn pickup_position(state: &WorldState, pk: &PickupState, mv: &dishonored_ir::gameplay::MovementConfig) -> Vec3 {
    match pk.carried_by.and_then(|c| state.player(c)) {
        Some(c) => c.position - Vec3::Z * (crate::movement::half_height(c, mv) * 0.2),
        None => pk.position,
    }
}

/// Use (F) on a pickup: returns true when something was taken.
pub fn try_use_pickup(state: &mut WorldState, who: usize, world: &dyn WorldQuery, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) -> bool {
    let p = state.players[who];
    let Some(pi) = pickup_in_view(state, &p, world, cfg) else { return false };
    if !can_take(&p, &state.pickups[pi], cfg) {
        return false;
    }
    let victim = state.pickups[pi].carried_by;
    let id = state.pickups[pi].id;
    take(state, pi, who, cfg, events);
    if let Some(v) = victim {
        events.push(SimEvent::Pickpocketed { id: p.id, victim: v, pickup: id });
    }
    true
}

/// Carried pickups follow their carrier; a dead carrier drops it where it lies.
pub fn update_carried(state: &mut WorldState, mv: &dishonored_ir::gameplay::MovementConfig) {
    for i in 0..state.pickups.len() {
        let Some(c) = state.pickups[i].carried_by else { continue };
        let pos = pickup_position(state, &state.pickups[i], mv);
        state.pickups[i].position = pos;
        if state.player(c).is_none() {
            state.pickups[i].carried_by = None;
        }
    }
}
