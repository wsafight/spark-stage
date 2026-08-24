use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptBundle {
    pub schema_version: String,
    pub project: ProjectSpec,
    pub bible: Bible,
    pub story: Story,
    pub shots: Vec<ShotContract>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authoring: Option<AuthoringMetadata>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectSpec {
    pub id: String,
    pub title: String,
    pub logline: String,
    pub genre: String,
    pub language: String,
    pub target_duration_seconds: u32,
    pub shot_count: usize,
    pub delivery: DeliverySpec,
    #[serde(default)]
    pub content_boundaries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeliverySpec {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Bible {
    pub characters: Vec<Character>,
    pub locations: Vec<Location>,
    pub style: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Character {
    pub id: String,
    pub name: String,
    pub age: u8,
    pub fictional: bool,
    pub appearance: String,
    pub wardrobe: String,
    pub personality: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Story {
    pub synopsis: String,
    pub beats: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShotContract {
    pub schema_version: String,
    pub id: String,
    pub title: String,
    pub duration: u32,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub operation: Operation,
    pub characters: Vec<String>,
    pub location: String,
    pub camera: Camera,
    pub conditioning: Option<Conditioning>,
    pub continuity: Continuity,
    pub generation_plan: GenerationPlan,
    #[serde(default)]
    pub dialogue: Vec<DialogueLine>,
    pub prompt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    T2v,
    I2v,
    Flf2v,
    R2v,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    pub shot_size: String,
    pub movement: String,
    #[serde(default)]
    pub screen_direction: BTreeMap<String, ScreenDirection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScreenDirection {
    Left,
    Right,
    Center,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Conditioning {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_frame: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_frame: Option<String>,
    #[serde(default)]
    pub reference_images: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_video: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Continuity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub relation: ContinuityRelation,
    pub handoff: Handoff,
    #[serde(default)]
    pub must_match: Vec<ContinuityAttribute>,
    #[serde(default)]
    pub state_in: BTreeMap<String, String>,
    #[serde(default)]
    pub state_out: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContinuityRelation {
    Continuous,
    Cut,
    NewScene,
    TimeJump,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Handoff {
    None,
    StableFrame,
    ApprovedFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContinuityAttribute {
    Wardrobe,
    Location,
    PropState,
    CharacterPosition,
    Lighting,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GenerationPlan {
    pub risk: Risk,
    pub audition_takes: u8,
    pub audition_profile: String,
    pub final_profile: String,
    pub promotion: Promotion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Promotion {
    Auto,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DialogueLine {
    pub who: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthoringMetadata {
    pub skill: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}
