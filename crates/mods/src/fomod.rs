//! FOMOD installers: `fomod/ModuleConfig.xml` in a mod archive describes
//! steps of choices (which textures, which patches) and the files each
//! choice installs. [`parse`] reads the XML into [`ModuleConfig`]; the UI
//! asks the user through [`Choices`]; [`ModuleConfig::files`] lists what to
//! install and [`install`] copies it.
//!
//! Only data is read from the file: the scripted (C#) installers of old
//! FOMM are not run. File dependencies are answered by the caller (usually
//! "does the game folder or an enabled mod have this file").

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use roxmltree::{Document, Node, ParsingOptions};

use crate::fsutil::{find_path_ci, join_rel, walk_files};

/// A parsed `ModuleConfig.xml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleConfig {
    pub name: String,
    /// Image path relative to the installer's root.
    pub image: Option<String>,
    /// Files every choice installs.
    pub required: Vec<FileItem>,
    pub steps: Vec<Step>,
    /// Files installed when flags and files match.
    pub conditional: Vec<(Condition, Vec<FileItem>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub name: String,
    /// Shown only when this holds.
    pub visible: Condition,
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub kind: GroupKind,
    pub plugins: Vec<Plugin>,
}

/// How many options of a group the user picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    AtLeastOne,
    AtMostOne,
    ExactlyOne,
    /// Every option, fixed.
    All,
    Any,
}

impl GroupKind {
    /// One choice at a time (radio buttons).
    pub fn single(self) -> bool {
        matches!(self, GroupKind::AtMostOne | GroupKind::ExactlyOne)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    pub name: String,
    pub description: String,
    pub image: Option<String>,
    pub files: Vec<FileItem>,
    /// Flags set when chosen.
    pub flags: Vec<(String, String)>,
    pub kind: PluginKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginKind {
    Fixed(PluginType),
    /// The first matching pattern's type, else `default`.
    Depends {
        default: PluginType,
        patterns: Vec<(Condition, PluginType)>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginType {
    Required,
    Optional,
    Recommended,
    NotUsable,
    CouldBeUsable,
}

/// A file or folder an option installs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileItem {
    /// Relative to the installer's root, `/`-separated.
    pub source: String,
    /// Relative to the mod's root, `/`-separated (`""` is the root).
    pub destination: String,
    pub folder: bool,
    /// Higher wins when two items write the same file.
    pub priority: i32,
    /// Installed whether or not its option is chosen.
    pub always_install: bool,
    /// Installed whenever its option is usable.
    pub install_if_usable: bool,
}

/// A condition on flags and files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    Always,
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Flag { name: String, value: String },
    File { path: String, state: FileState },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    Active,
    Inactive,
    Missing,
}

impl Condition {
    pub fn holds(
        &self,
        flags: &HashMap<String, String>,
        files: &dyn Fn(&str) -> FileState,
    ) -> bool {
        match self {
            Condition::Always => true,
            Condition::All(c) => c.iter().all(|c| c.holds(flags, files)),
            Condition::Any(c) => c.is_empty() || c.iter().any(|c| c.holds(flags, files)),
            Condition::Flag { name, value } => {
                flags.get(name).map_or(value.is_empty(), |v| v == value)
            }
            Condition::File { path, state } => files(path) == *state,
        }
    }
}

/// Where the installer is in an unpacked archive: the folder holding
/// `fomod/ModuleConfig.xml` (any letter case), searched two levels deep.
pub fn find(root: &Path) -> Option<(PathBuf, PathBuf)> {
    fn search(dir: &Path, depth: usize) -> Option<(PathBuf, PathBuf)> {
        if let Some(config) = find_path_ci(dir, "fomod/ModuleConfig.xml")
            && config.is_file()
        {
            return Some((dir.to_path_buf(), config));
        }
        if depth == 0 {
            return None;
        }
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect();
        dirs.sort();
        dirs.iter().find_map(|d| search(d, depth - 1))
    }
    search(root, 2)
}

/// Decode `ModuleConfig.xml`, which is UTF-16 as often as UTF-8.
fn decode(bytes: &[u8]) -> Result<String> {
    let utf16 = |data: &[u8], le: bool| -> Result<String> {
        let units: Vec<u16> = data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| {
                if le {
                    u16::from_le_bytes(c)
                } else {
                    u16::from_be_bytes(c)
                }
            })
            .collect();
        String::from_utf16(&units).context("the installer's text is damaged")
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => Ok(String::from_utf8_lossy(rest).into_owned()),
        // `<` without a byte order mark.
        [b'<', 0, ..] => utf16(bytes, true),
        [0, b'<', ..] => utf16(bytes, false),
        _ => Ok(String::from_utf8_lossy(bytes).into_owned()),
    }
}

/// Parse `ModuleConfig.xml`.
pub fn parse(bytes: &[u8]) -> Result<ModuleConfig> {
    let text = decode(bytes)?;
    // Some installers declare their encoding as UTF-16 after being saved as
    // UTF-8; the text is already decoded, so drop the declaration.
    let text = strip_declaration(&text);
    let doc = Document::parse_with_options(
        text,
        ParsingOptions {
            allow_dtd: false,
            ..Default::default()
        },
    )
    .context("the installer's ModuleConfig.xml is not valid XML")?;
    let root = doc.root_element();
    if !root.tag_name().name().eq_ignore_ascii_case("config") {
        bail!("ModuleConfig.xml does not describe an installer");
    }
    let mut config = ModuleConfig {
        name: child(root, "moduleName")
            .map(text_of)
            .unwrap_or_default()
            .trim()
            .to_string(),
        image: child(root, "moduleImage").and_then(|n| attr(n, "path")),
        ..Default::default()
    };
    if let Some(req) = child(root, "requiredInstallFiles") {
        config.required = file_list(req)?;
    }
    if let Some(steps) = child(root, "installSteps") {
        let mut list = Vec::new();
        for step in children(steps, "installStep") {
            let mut groups = Vec::new();
            if let Some(g) = child(step, "optionalFileGroups") {
                for group in children(g, "group") {
                    groups.push(parse_group(group)?);
                }
                sort_by_order(g, &mut groups, |g| g.name.clone());
            }
            list.push(Step {
                name: attr(step, "name").unwrap_or_default(),
                visible: child(step, "visible")
                    .map(condition_of)
                    .unwrap_or(Condition::Always),
                groups,
            });
        }
        sort_by_order(steps, &mut list, |s| s.name.clone());
        config.steps = list;
    }
    if let Some(cond) = child(root, "conditionalFileInstalls")
        && let Some(patterns) = child(cond, "patterns")
    {
        for pattern in children(patterns, "pattern") {
            let when = child(pattern, "dependencies")
                .map(condition_of)
                .unwrap_or(Condition::Always);
            let files = match child(pattern, "files") {
                Some(f) => file_list(f)?,
                None => Vec::new(),
            };
            config.conditional.push((when, files));
        }
    }
    Ok(config)
}

fn strip_declaration(text: &str) -> &str {
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with("<?xml")
        && let Some(end) = trimmed.find("?>")
    {
        return &trimmed[end + 2..];
    }
    trimmed
}

fn parse_group(group: Node) -> Result<Group> {
    let kind = match attr(group, "type").as_deref() {
        Some("SelectAtLeastOne") => GroupKind::AtLeastOne,
        Some("SelectAtMostOne") => GroupKind::AtMostOne,
        Some("SelectExactlyOne") => GroupKind::ExactlyOne,
        Some("SelectAll") => GroupKind::All,
        _ => GroupKind::Any,
    };
    let mut plugins = Vec::new();
    if let Some(list) = child(group, "plugins") {
        for p in children(list, "plugin") {
            plugins.push(parse_plugin(p)?);
        }
        sort_by_order(list, &mut plugins, |p| p.name.clone());
    }
    Ok(Group {
        name: attr(group, "name").unwrap_or_default(),
        kind,
        plugins,
    })
}

fn parse_plugin(p: Node) -> Result<Plugin> {
    let flags = child(p, "conditionFlags")
        .map(|f| {
            children(f, "flag")
                .map(|flag| {
                    (
                        attr(flag, "name").unwrap_or_default(),
                        text_of(flag).trim().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let kind = match child(p, "typeDescriptor") {
        Some(desc) => match child(desc, "dependencyType") {
            Some(dep) => PluginKind::Depends {
                default: child(dep, "defaultType")
                    .map(type_of)
                    .unwrap_or(PluginType::Optional),
                patterns: child(dep, "patterns")
                    .map(|ps| {
                        children(ps, "pattern")
                            .map(|pat| {
                                (
                                    child(pat, "dependencies")
                                        .map(condition_of)
                                        .unwrap_or(Condition::Always),
                                    child(pat, "type")
                                        .map(type_of)
                                        .unwrap_or(PluginType::Optional),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            None => PluginKind::Fixed(
                child(desc, "type")
                    .map(type_of)
                    .unwrap_or(PluginType::Optional),
            ),
        },
        None => PluginKind::Fixed(PluginType::Optional),
    };
    Ok(Plugin {
        name: attr(p, "name").unwrap_or_default(),
        description: child(p, "description")
            .map(text_of)
            .unwrap_or_default()
            .trim()
            .to_string(),
        image: child(p, "image").and_then(|n| attr(n, "path")),
        files: match child(p, "files") {
            Some(f) => file_list(f)?,
            None => Vec::new(),
        },
        flags,
        kind,
    })
}

fn type_of(node: Node) -> PluginType {
    match attr(node, "name").as_deref() {
        Some("Required") => PluginType::Required,
        Some("Recommended") => PluginType::Recommended,
        Some("NotUsable") => PluginType::NotUsable,
        Some("CouldBeUsable") => PluginType::CouldBeUsable,
        _ => PluginType::Optional,
    }
}

/// A `<visible>`/`<dependencies>` element: its operator over its children.
fn condition_of(node: Node) -> Condition {
    let parts: Vec<Condition> = node
        .children()
        .filter(Node::is_element)
        .filter_map(|c| match c.tag_name().name() {
            "flagDependency" => Some(Condition::Flag {
                name: attr(c, "flag").unwrap_or_default(),
                value: attr(c, "value").unwrap_or_default(),
            }),
            "fileDependency" => Some(Condition::File {
                path: attr(c, "file").unwrap_or_default().replace('\\', "/"),
                state: match attr(c, "state").as_deref() {
                    Some("Missing") => FileState::Missing,
                    Some("Inactive") => FileState::Inactive,
                    _ => FileState::Active,
                },
            }),
            "dependencies" => Some(condition_of(c)),
            // Game, script extender and manager versions: assume met.
            _ => None,
        })
        .collect();
    match attr(node, "operator").as_deref() {
        Some("Or") => Condition::Any(parts),
        _ => Condition::All(parts),
    }
}

fn file_list(node: Node) -> Result<Vec<FileItem>> {
    let mut out = Vec::new();
    for c in node.children().filter(Node::is_element) {
        let folder = match c.tag_name().name() {
            "file" => false,
            "folder" => true,
            _ => continue,
        };
        let source = clean(&attr(c, "source").unwrap_or_default());
        if source.is_empty() && !folder {
            bail!("the installer names a file without a source");
        }
        let destination = match attr(c, "destination") {
            // A file sent to a folder (`textures\`) keeps its name.
            Some(d) if !folder && (d.ends_with('/') || d.ends_with('\\')) => {
                let name = source.rsplit('/').next().unwrap_or_default();
                clean(&format!("{}/{name}", clean(&d)))
            }
            Some(d) => clean(&d),
            None if folder => String::new(),
            None => source.clone(),
        };
        for path in [&source, &destination] {
            if !path.is_empty()
                && irori_core::archive_guard::relative_entry_path(Path::new(path)).is_none()
            {
                bail!("the installer names an unsafe path: {path}");
            }
        }
        out.push(FileItem {
            source,
            destination,
            folder,
            priority: attr(c, "priority")
                .and_then(|p| p.parse().ok())
                .unwrap_or(0),
            always_install: attr(c, "alwaysInstall").as_deref() == Some("true"),
            install_if_usable: attr(c, "installIfUsable").as_deref() == Some("true"),
        });
    }
    Ok(out)
}

/// `\` to `/`, without leading or trailing separators.
fn clean(path: &str) -> String {
    path.replace('\\', "/").trim_matches('/').to_string()
}

/// `order="Ascending"` (the default) and `"Descending"` sort by name;
/// `"Explicit"` keeps the file's order.
fn sort_by_order<T>(node: Node, items: &mut [T], name: impl Fn(&T) -> String) {
    match attr(node, "order").as_deref() {
        Some("Explicit") => {}
        Some("Descending") => items.sort_by_key(|i| std::cmp::Reverse(name(i).to_lowercase())),
        _ => items.sort_by_key(|i| name(i).to_lowercase()),
    }
}

fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn children<'a, 'i: 'a>(node: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> {
    node.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

fn attr(node: Node, name: &str) -> Option<String> {
    node.attribute(name).map(str::to_string)
}

fn text_of(node: Node) -> String {
    node.descendants()
        .filter(Node::is_text)
        .filter_map(|n| n.text())
        .collect()
}

/// What the user picked: for each step, group and option, whether it is
/// chosen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Choices(pub Vec<Vec<Vec<bool>>>);

impl Choices {
    pub fn is_chosen(&self, step: usize, group: usize, plugin: usize) -> bool {
        self.0
            .get(step)
            .and_then(|s| s.get(group))
            .and_then(|g| g.get(plugin))
            .copied()
            .unwrap_or(false)
    }
}

/// Answers file conditions: whether a file is present and active.
pub type FileCheck<'a> = &'a dyn Fn(&str) -> FileState;

impl ModuleConfig {
    /// The type of `plugin` given the flags set so far.
    pub fn plugin_type(
        plugin: &Plugin,
        flags: &HashMap<String, String>,
        files: FileCheck,
    ) -> PluginType {
        match &plugin.kind {
            PluginKind::Fixed(t) => *t,
            PluginKind::Depends { default, patterns } => patterns
                .iter()
                .find(|(c, _)| c.holds(flags, files))
                .map_or(*default, |(_, t)| *t),
        }
    }

    /// Flags set by the options chosen in the visible steps before `step`.
    pub fn flags_before(
        &self,
        step: usize,
        choices: &Choices,
        files: FileCheck,
    ) -> HashMap<String, String> {
        let mut flags = HashMap::new();
        for (s, st) in self.steps.iter().enumerate().take(step) {
            if !st.visible.holds(&flags, files) {
                continue;
            }
            for (g, group) in st.groups.iter().enumerate() {
                for (p, plugin) in group.plugins.iter().enumerate() {
                    if choices.is_chosen(s, g, p) {
                        for (k, v) in &plugin.flags {
                            flags.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
        }
        flags
    }

    pub fn step_visible(&self, step: usize, choices: &Choices, files: FileCheck) -> bool {
        self.steps.get(step).is_some_and(|s| {
            s.visible
                .holds(&self.flags_before(step, choices, files), files)
        })
    }

    /// Default picks for step `step` given what came before: required and
    /// recommended options, every option of "all" groups, and the first
    /// usable option where one is needed.
    pub fn default_step(&self, step: usize, choices: &Choices, files: FileCheck) -> Vec<Vec<bool>> {
        let flags = self.flags_before(step, choices, files);
        let Some(st) = self.steps.get(step) else {
            return Vec::new();
        };
        st.groups
            .iter()
            .map(|group| {
                let types: Vec<PluginType> = group
                    .plugins
                    .iter()
                    .map(|p| Self::plugin_type(p, &flags, files))
                    .collect();
                let mut picked: Vec<bool> = types
                    .iter()
                    .map(|t| {
                        group.kind == GroupKind::All
                            || matches!(t, PluginType::Required | PluginType::Recommended)
                    })
                    .collect();
                if group.kind.single() && picked.iter().filter(|p| **p).count() > 1 {
                    let first = picked.iter().position(|p| *p);
                    picked = (0..picked.len()).map(|i| Some(i) == first).collect();
                }
                let needs_one = matches!(group.kind, GroupKind::ExactlyOne | GroupKind::AtLeastOne);
                if needs_one && !picked.contains(&true) {
                    let usable = types.iter().position(|t| *t != PluginType::NotUsable);
                    if let Some(ix) = usable {
                        picked[ix] = true;
                    }
                }
                picked
            })
            .collect()
    }

    /// Defaults for every step, in order, each seeing the steps before it.
    pub fn default_choices(&self, files: FileCheck) -> Choices {
        let mut choices = Choices::default();
        for step in 0..self.steps.len() {
            let picks = self.default_step(step, &choices, files);
            choices.0.push(picks);
        }
        choices
    }

    /// Whether the picks of `group` in `step` satisfy the group's rule.
    pub fn group_valid(&self, step: usize, group: usize, choices: &Choices) -> bool {
        let Some(g) = self.steps.get(step).and_then(|s| s.groups.get(group)) else {
            return true;
        };
        let n = (0..g.plugins.len())
            .filter(|p| choices.is_chosen(step, group, *p))
            .count();
        match g.kind {
            GroupKind::ExactlyOne => n == 1 || g.plugins.is_empty(),
            GroupKind::AtMostOne => n <= 1,
            GroupKind::AtLeastOne => n >= 1 || g.plugins.is_empty(),
            GroupKind::All | GroupKind::Any => true,
        }
    }

    /// Files to install for these choices, lowest priority first.
    pub fn files(&self, choices: &Choices, files: FileCheck) -> Vec<FileItem> {
        let mut out: Vec<FileItem> = self.required.clone();
        let mut flags = HashMap::new();
        for (s, st) in self.steps.iter().enumerate() {
            if !st.visible.holds(&flags, files) {
                continue;
            }
            let mut set = Vec::new();
            for (g, group) in st.groups.iter().enumerate() {
                for (p, plugin) in group.plugins.iter().enumerate() {
                    let chosen = choices.is_chosen(s, g, p);
                    let usable = Self::plugin_type(plugin, &flags, files) != PluginType::NotUsable;
                    for item in &plugin.files {
                        if chosen || item.always_install || (item.install_if_usable && usable) {
                            out.push(item.clone());
                        }
                    }
                    if chosen {
                        set.extend(plugin.flags.iter().cloned());
                    }
                }
            }
            flags.extend(set);
        }
        for (when, items) in &self.conditional {
            if when.holds(&flags, files) {
                out.extend(items.iter().cloned());
            }
        }
        // Stable: equal priorities keep the installer's order.
        out.sort_by_key(|i| i.priority);
        out
    }
}

/// Copy `items` from the installer's `root` into `tree`. Later items
/// replace files of earlier ones. Returns the number of files written.
pub fn install(root: &Path, items: &[FileItem], tree: &Path) -> Result<usize> {
    let mut written = 0;
    for item in items {
        let Some(source) = find_path_ci(root, &item.source) else {
            bail!("the installer's file {} is missing", item.source);
        };
        if item.folder {
            if !source.is_dir() {
                bail!("the installer's folder {} is missing", item.source);
            }
            for (rel, _) in walk_files(&source)? {
                let dest = if item.destination.is_empty() {
                    rel.clone()
                } else {
                    format!("{}/{rel}", item.destination)
                };
                copy_into(&join_rel(&source, &rel), tree, &dest)?;
                written += 1;
            }
        } else {
            let dest = if item.destination.is_empty() {
                source
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            } else {
                item.destination.clone()
            };
            copy_into(&source, tree, &dest)?;
            written += 1;
        }
    }
    Ok(written)
}

fn copy_into(source: &Path, tree: &Path, rel: &str) -> Result<()> {
    if irori_core::archive_guard::relative_entry_path(Path::new(rel)).is_none() {
        bail!("the installer names an unsafe path: {rel}");
    }
    let target = join_rel(tree, rel);
    if let Some(parent) = target.parent() {
        irori_core::archive_guard::ensure_no_symlinks(
            tree,
            parent.strip_prefix(tree).unwrap_or(Path::new("")),
        )?;
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, &target)
        .with_context(|| format!("copying {} to {}", source.display(), target.display()))?;
    Ok(())
}

/// The file of an installer image (`fomod\images\a.png`, any letter case)
/// under the installer's `root`.
pub fn image_path(root: &Path, rel: &str) -> Option<PathBuf> {
    find_path_ci(root, rel).filter(|p| p.is_file())
}
