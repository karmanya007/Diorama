#![allow(clippy::unwrap_used, reason = "test")]

use std::collections::BTreeMap;

use lumen_scene::particles::*;
use lumen_scene::{Blend, Pass, ProgramId};

use super::*;

fn range(min: f32, max: f32) -> Range<f32> {
    Range {
        min,
        max,
        exponent: 1.0,
    }
}

fn one_shot() -> Emitter {
    Emitter::Box {
        origin: [0.0; 3],
        min: [0.0; 3],
        max: [0.0; 3],
        directions: [0.0; 3],
        speed: [0.0; 2],
        rate: 0.0,
        instantaneous: 1,
        duration: 0.0,
        audio: false,
    }
}

fn system(rate: f32, operators: Vec<Operator>) -> ParticleSystem {
    ParticleSystem {
        draw: Pass {
            program: ProgramId(0),
            textures: BTreeMap::new(),
            target: None,
            constants: BTreeMap::new(),
            blend: Blend::Additive,
        },
        max_count: 100,
        start_time: 0.0,
        emitters: vec![Emitter::Sphere {
            origin: [0.0; 3],
            directions: [1.0, 1.0, 0.0],
            distance: [10.0, 10.0],
            speed: [0.0, 0.0],
            rate,
            instantaneous: 0,
            duration: 0.0,
            audio: false,
        }],
        initializers: vec![
            Initializer::Lifetime(range(2.0, 2.0)),
            Initializer::Size(range(8.0, 8.0)),
        ],
        operators,
        renderer: ParticleRenderer::Sprite,
        control_points: Vec::new(),
        children: Vec::new(),
        sprite_sheet: None,
        perspective: false,
        instance: InstanceOverride::default(),
    }
}

#[test]
fn emits_at_rate_and_retires_at_lifetime() {
    let mut sim = Simulation::new(&system(10.0, Vec::new()), 1);
    sim.advance_to(1.0);
    let n = sim.particles(0).len();
    assert!((9..=11).contains(&n), "{n}");
    // Lifetime 2 s: steady state is rate × lifetime.
    sim.advance_to(5.0);
    let n = sim.particles(0).len();
    assert!((18..=21).contains(&n), "{n}");
    // Sphere emitter at distance 10 in the xy plane.
    for p in sim.particles(0) {
        let r = (p.position[0].powi(2) + p.position[1].powi(2)).sqrt();
        assert!((r - 10.0).abs() < 1e-3 && p.position[2] == 0.0, "{p:?}");
    }
}

#[test]
fn instance_overrides_scale_count_and_size() {
    let mut s = system(1000.0, Vec::new());
    s.instance.count = 0.1;
    s.instance.size = 0.5;
    let mut sim = Simulation::new(&s, 1);
    sim.advance_to(1.0);
    assert_eq!(sim.particles(0).len(), 10);
    assert!(sim.particles(0).iter().all(|p| p.size == 4.0));
}

#[test]
fn movement_needs_its_operator() {
    let mut still = system(0.0, Vec::new());
    still.emitters = vec![one_shot()];
    let mut falling = still.clone();
    falling.operators = vec![Operator::Movement {
        gravity: [0.0, -100.0, 0.0],
        drag: 0.0,
    }];
    let y = |s: &ParticleSystem| {
        let mut sim = Simulation::new(s, 3);
        sim.advance_to(1.0);
        sim.particles(0)[0].position[1]
    };
    assert_eq!(y(&still), 0.0);
    // v = −100 t → y ≈ −50 t² (fixed steps).
    assert!((y(&falling) + 50.0).abs() < 3.0, "{}", y(&falling));
}

#[test]
fn alpha_fades_in_and_out() {
    let mut s = system(
        0.0,
        vec![Operator::AlphaFade {
            fade_in: 0.25,
            fade_out: 0.75,
        }],
    );
    s.emitters = vec![one_shot()];
    let mut sim = Simulation::new(&s, 5);
    sim.advance_to(0.25); // life 0.125: half faded in
    assert!((sim.particles(0)[0].alpha - 0.5).abs() < 0.05);
    sim.advance_to(1.0); // life 0.5: full
    assert!((sim.particles(0)[0].alpha - 1.0).abs() < 1e-3);
    sim.advance_to(1.75); // life 0.875: half faded out
    assert!((sim.particles(0)[0].alpha - 0.5).abs() < 0.05);
}

#[test]
fn deterministic_for_a_seed() {
    let s = system(
        30.0,
        vec![Operator::Turbulence {
            scale: 0.01,
            timescale: 1.0,
            speed: [50.0, 100.0],
            mask: [1.0, 1.0, 0.0],
            phase: 0.0,
        }],
    );
    let run = |seed| {
        let mut sim = Simulation::new(&s, seed);
        sim.advance_to(3.0);
        sim.particles(0).to_vec()
    };
    assert_eq!(run(7), run(7));
    assert_ne!(run(7), run(8));
}

#[test]
fn follow_children_emit_at_parent_particles() {
    let mut parent = system(5.0, Vec::new());
    parent.children = vec![ParticleChild {
        kind: ChildKind::Follow,
        origin: [0.0; 3],
        scale: [1.0; 3],
        probability: 1.0,
        system: system(10.0, Vec::new()),
    }];
    let mut sim = Simulation::new(&parent, 1);
    sim.advance_to(1.0);
    assert_eq!(sim.system_count(), 2);
    assert!(!sim.particles(1).is_empty());
    // Child particles start 10 px from a parent particle (sphere emitter);
    // parents stand still (no movement operator).
    let parents: Vec<_> = sim
        .particles(0)
        .iter()
        .map(Particle::draw_position)
        .collect();
    for c in sim.particles(1) {
        assert!(
            parents.iter().any(|p| {
                ((c.position[0] - p[0]).powi(2) + (c.position[1] - p[1]).powi(2)).sqrt() < 10.5
            }),
            "{c:?}"
        );
    }
}
