//! The generative leak differential (tests/render_leak_oracle.rs): a
//! seeded document generator that plants markers, bounded Emacs as the
//! oracle of what Org's export shows, every mae-org output channel, the
//! verdict, and a delta-debugging minimiser.

pub mod channels;
pub mod emacs;
pub mod fixed;
pub mod gen;
pub mod gen_blocks;
pub mod gen_doc;
pub mod gen_objects;
pub mod judge;
pub mod minimise;
pub mod report;
pub mod view;
