pub mod communication;
pub mod control_function_tests;
pub mod deployment;
#[cfg(feature = "legacy-native")]
pub mod desktop;
pub mod engineering;
pub mod kalman_tuning;
pub mod linear_mpc;
pub mod logic;
pub mod model;
pub mod online_optimization;
pub mod parameter_optimization;
pub mod project;
pub mod project_config;
pub mod runtime;
pub mod simulation;
pub mod storage;
pub mod strategy_service;
pub mod types;
pub mod virtual_hardware;
pub mod watchdog;
pub mod web;
mod web_api;
mod web_assets;

pub mod controller_signals;
