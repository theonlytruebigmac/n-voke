//! Agent migration: moving Windows devices from one N-central server to another
//! (or reinstalling on the same one). The design is in docs/migration/. Everything
//! here is logic that doesn't touch the network, so it can be tested in place.

pub mod ecoverse;
pub mod endpoint_result;
pub mod actions;
pub mod connect;
pub mod finish;
pub mod launcher;
pub mod launchview;
pub mod mapping;
pub mod mapview;
pub mod packager;
pub mod planner;
pub mod planview;
pub mod prepare;
pub mod poller;
pub mod project;
pub mod report;
pub mod store;
pub mod retire;
pub mod settings;
pub mod structure;
pub mod survey;
pub mod tracker;
pub mod trackview;
pub mod workflow;
