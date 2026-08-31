//! Safe Rust 实现的 Adaptive Radix Tree。
//!
//! 节点按实际扇出在 Node4/16/48/256 之间扩缩，叶端保存完整键以处理前缀终止和
//! 防止压缩路径误命中。遍历按无符号字节序稳定输出，因此既可服务单字段范围/前缀
//! 查询，也可承载复合 PropertyKey；实现不使用裸指针或 unsafe。

use std::cmp::Ordering;

#[derive(Debug, Clone)]
struct Terminal<V> {
    key: Vec<u8>,
    value: V,
}

#[derive(Debug, Clone)]
struct SmallNode<V, const CAPACITY: usize> {
    keys: [u8; CAPACITY],
    children: [Option<Box<ArtNode<V>>>; CAPACITY],
    len: usize,
}

impl<V, const CAPACITY: usize> Default for SmallNode<V, CAPACITY> {
    fn default() -> Self {
        Self {
            keys: [0; CAPACITY],
            children: std::array::from_fn(|_| None),
            len: 0,
        }
    }
}

impl<V, const CAPACITY: usize> SmallNode<V, CAPACITY> {
    fn position(&self, edge: u8) -> Result<usize, usize> {
        self.keys[..self.len].binary_search(&edge)
    }

    fn insert(&mut self, edge: u8, child: ArtNode<V>) {
        match self.position(edge) {
            Ok(position) => self.children[position] = Some(Box::new(child)),
            Err(position) => {
                for index in (position..self.len).rev() {
                    self.keys[index + 1] = self.keys[index];
                    self.children[index + 1] = self.children[index].take();
                }
                self.keys[position] = edge;
                self.children[position] = Some(Box::new(child));
                self.len += 1;
            }
        }
    }

    fn remove(&mut self, edge: u8) -> Option<Box<ArtNode<V>>> {
        let position = self.position(edge).ok()?;
        let removed = self.children[position].take();
        for index in position + 1..self.len {
            self.keys[index - 1] = self.keys[index];
            self.children[index - 1] = self.children[index].take();
        }
        self.len -= 1;
        self.children[self.len] = None;
        removed
    }
}

fn set_occupied(bitmap: &mut [u64; 4], edge: u8, occupied: bool) {
    let word = usize::from(edge) / 64;
    let bit = u32::from(edge % 64);
    if occupied {
        bitmap[word] |= 1u64 << bit;
    } else {
        bitmap[word] &= !(1u64 << bit);
    }
}

#[derive(Debug, Clone)]
struct Node48<V> {
    index: Box<[u8; 256]>,
    occupied: [u64; 4],
    children: [Option<Box<ArtNode<V>>>; 48],
    len: usize,
}

impl<V> Default for Node48<V> {
    fn default() -> Self {
        Self {
            index: Box::new([0; 256]),
            occupied: [0; 4],
            children: std::array::from_fn(|_| None),
            len: 0,
        }
    }
}

impl<V> Node48<V> {
    fn slot(&self, edge: u8) -> Option<usize> {
        let encoded = self.index[usize::from(edge)];
        (encoded != 0).then(|| usize::from(encoded - 1))
    }

    fn insert(&mut self, edge: u8, child: ArtNode<V>) {
        if let Some(slot) = self.slot(edge) {
            self.children[slot] = Some(Box::new(child));
            return;
        }
        let slot = self
            .children
            .iter()
            .position(Option::is_none)
            .expect("Node48 插入前必须有空槽位");
        self.children[slot] = Some(Box::new(child));
        self.index[usize::from(edge)] = u8::try_from(slot + 1).expect("Node48 槽位必须可编码");
        set_occupied(&mut self.occupied, edge, true);
        self.len += 1;
    }

    fn remove(&mut self, edge: u8) -> Option<Box<ArtNode<V>>> {
        let slot = self.slot(edge)?;
        self.index[usize::from(edge)] = 0;
        set_occupied(&mut self.occupied, edge, false);
        self.len -= 1;
        self.children[slot].take()
    }
}

#[derive(Debug, Clone)]
struct Node256<V> {
    children: Box<[Option<Box<ArtNode<V>>>; 256]>,
    occupied: [u64; 4],
    len: usize,
}

impl<V> Default for Node256<V> {
    fn default() -> Self {
        Self {
            children: Box::new(std::array::from_fn(|_| None)),
            occupied: [0; 4],
            len: 0,
        }
    }
}

#[derive(Debug, Clone)]
enum Children<V> {
    Node4(SmallNode<V, 4>),
    Node16(SmallNode<V, 16>),
    Node48(Box<Node48<V>>),
    Node256(Node256<V>),
}

impl<V> Default for Children<V> {
    fn default() -> Self {
        Self::Node4(SmallNode::default())
    }
}

impl<V> Children<V> {
    fn len(&self) -> usize {
        match self {
            Self::Node4(node) => node.len,
            Self::Node16(node) => node.len,
            Self::Node48(node) => node.len,
            Self::Node256(node) => node.len,
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn get(&self, edge: u8) -> Option<&ArtNode<V>> {
        match self {
            Self::Node4(node) => node.children[node.position(edge).ok()?].as_deref(),
            Self::Node16(node) => node.children[node.position(edge).ok()?].as_deref(),
            Self::Node48(node) => node.children[node.slot(edge)?].as_deref(),
            Self::Node256(node) => node.children[usize::from(edge)].as_deref(),
        }
    }

    fn get_mut(&mut self, edge: u8) -> Option<&mut ArtNode<V>> {
        match self {
            Self::Node4(node) => node.children[node.position(edge).ok()?].as_deref_mut(),
            Self::Node16(node) => node.children[node.position(edge).ok()?].as_deref_mut(),
            Self::Node48(node) => {
                let slot = node.slot(edge)?;
                node.children[slot].as_deref_mut()
            }
            Self::Node256(node) => node.children[usize::from(edge)].as_deref_mut(),
        }
    }

    fn insert(&mut self, edge: u8, child: ArtNode<V>) {
        let needs_upgrade = match self {
            Self::Node4(node) => node.len == 4 && node.position(edge).is_err(),
            Self::Node16(node) => node.len == 16 && node.position(edge).is_err(),
            Self::Node48(node) => node.len == 48 && node.slot(edge).is_none(),
            Self::Node256(_) => false,
        };
        if needs_upgrade {
            self.rebuild(self.len() + 1);
        }
        match self {
            Self::Node4(node) => node.insert(edge, child),
            Self::Node16(node) => node.insert(edge, child),
            Self::Node48(node) => node.insert(edge, child),
            Self::Node256(node) => {
                let slot = &mut node.children[usize::from(edge)];
                if slot.is_none() {
                    node.len += 1;
                    set_occupied(&mut node.occupied, edge, true);
                }
                *slot = Some(Box::new(child));
            }
        }
    }

    fn remove(&mut self, edge: u8) {
        let removed = match self {
            Self::Node4(node) => node.remove(edge),
            Self::Node16(node) => node.remove(edge),
            Self::Node48(node) => node.remove(edge),
            Self::Node256(node) => {
                let removed = node.children[usize::from(edge)].take();
                if removed.is_some() {
                    node.len -= 1;
                    set_occupied(&mut node.occupied, edge, false);
                }
                removed
            }
        };
        if removed.is_some() {
            self.rebuild(self.len());
        }
    }

    fn rebuild(&mut self, target_len: usize) {
        let target_kind = match target_len {
            0..=4 => 4,
            5..=16 => 16,
            17..=48 => 48,
            _ => 256,
        };
        let current_kind = self.kind();
        if current_kind == target_kind {
            return;
        }
        let entries = std::mem::take(self).into_entries();
        *self = match target_kind {
            4 => Self::Node4(SmallNode::default()),
            16 => Self::Node16(SmallNode::default()),
            48 => Self::Node48(Box::default()),
            _ => Self::Node256(Node256::default()),
        };
        for (edge, child) in entries {
            self.insert(edge, *child);
        }
    }

    fn kind(&self) -> u16 {
        match self {
            Self::Node4(_) => 4,
            Self::Node16(_) => 16,
            Self::Node48(_) => 48,
            Self::Node256(_) => 256,
        }
    }

    fn into_entries(self) -> Vec<(u8, Box<ArtNode<V>>)> {
        let mut output = Vec::with_capacity(self.len());
        match self {
            Self::Node4(mut node) => {
                for index in 0..node.len {
                    output.push((
                        node.keys[index],
                        node.children[index].take().expect("Node4 槽位必须存在"),
                    ));
                }
            }
            Self::Node16(mut node) => {
                for index in 0..node.len {
                    output.push((
                        node.keys[index],
                        node.children[index].take().expect("Node16 槽位必须存在"),
                    ));
                }
            }
            Self::Node48(mut node) => {
                for edge in 0u8..=u8::MAX {
                    if let Some(slot) = node.slot(edge) {
                        output.push((
                            edge,
                            node.children[slot].take().expect("Node48 槽位必须存在"),
                        ));
                    }
                }
            }
            Self::Node256(mut node) => {
                for edge in 0u8..=u8::MAX {
                    if let Some(child) = node.children[usize::from(edge)].take() {
                        output.push((edge, child));
                    }
                }
            }
        }
        output
    }

    fn iter(&self, descending: bool) -> ChildIter<'_, V> {
        let next = if descending { 255 } else { 0 };
        let kind = match self {
            Self::Node4(node) => ChildIterKind::Node4(node),
            Self::Node16(node) => ChildIterKind::Node16(node),
            Self::Node48(node) => ChildIterKind::Node48(node),
            Self::Node256(node) => ChildIterKind::Node256(node),
        };
        let next = match self {
            Self::Node4(node) => {
                if descending {
                    node.len as i16 - 1
                } else {
                    0
                }
            }
            Self::Node16(node) => {
                if descending {
                    node.len as i16 - 1
                } else {
                    0
                }
            }
            Self::Node48(_) | Self::Node256(_) => next,
        };
        let occupied = match self {
            Self::Node48(node) => node.occupied,
            Self::Node256(node) => node.occupied,
            Self::Node4(_) | Self::Node16(_) => [0; 4],
        };
        ChildIter {
            kind,
            next,
            descending,
            occupied,
        }
    }

    fn first(&self) -> Option<(u8, &ArtNode<V>)> {
        self.iter(false).next()
    }

    fn last(&self) -> Option<(u8, &ArtNode<V>)> {
        self.iter(true).next()
    }

    fn take_only_child(&mut self) -> Option<(u8, Box<ArtNode<V>>)> {
        if self.len() != 1 {
            return None;
        }
        std::mem::take(self).into_entries().pop()
    }
}

struct ChildIter<'a, V> {
    kind: ChildIterKind<'a, V>,
    next: i16,
    descending: bool,
    occupied: [u64; 4],
}

enum ChildIterKind<'a, V> {
    Node4(&'a SmallNode<V, 4>),
    Node16(&'a SmallNode<V, 16>),
    Node48(&'a Node48<V>),
    Node256(&'a Node256<V>),
}

impl<'a, V> ChildIter<'a, V> {
    fn next_dense_position(&mut self) -> Option<i16> {
        if self.descending {
            if self.next < 0 {
                return None;
            }
            let edge = self.next as usize;
            let word = edge / 64;
            let bit = edge % 64;
            let masked = self.occupied[word] & (u64::MAX >> (63 - bit));
            if masked != 0 {
                let found = word * 64 + (63 - masked.leading_zeros() as usize);
                self.next = found as i16 - 1;
                return Some(found as i16);
            }
            for word in (0..word).rev() {
                let bits = self.occupied[word];
                if bits != 0 {
                    let found = word * 64 + (63 - bits.leading_zeros() as usize);
                    self.next = found as i16 - 1;
                    return Some(found as i16);
                }
            }
            None
        } else {
            if self.next > 255 {
                return None;
            }
            let edge = self.next as usize;
            let word = edge / 64;
            let bit = edge % 64;
            let masked = self.occupied[word] & (u64::MAX << bit);
            if masked != 0 {
                let found = word * 64 + masked.trailing_zeros() as usize;
                self.next = found as i16 + 1;
                return Some(found as i16);
            }
            for word in word + 1..4 {
                let bits = self.occupied[word];
                if bits != 0 {
                    let found = word * 64 + bits.trailing_zeros() as usize;
                    self.next = found as i16 + 1;
                    return Some(found as i16);
                }
            }
            None
        }
    }
}

impl<'a, V> Iterator for ChildIter<'a, V> {
    type Item = (u8, &'a ArtNode<V>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let dense = matches!(
                self.kind,
                ChildIterKind::Node48(_) | ChildIterKind::Node256(_)
            );
            let position = if dense {
                self.next_dense_position()?
            } else {
                let position = self.next;
                self.next += if self.descending { -1 } else { 1 };
                position
            };
            match &self.kind {
                ChildIterKind::Node4(node) => {
                    let node = *node;
                    if position < 0 || position as usize >= node.len {
                        return None;
                    }
                    let position = position as usize;
                    return Some((
                        node.keys[position],
                        node.children[position]
                            .as_deref()
                            .expect("Node4 槽位必须存在"),
                    ));
                }
                ChildIterKind::Node16(node) => {
                    let node = *node;
                    if position < 0 || position as usize >= node.len {
                        return None;
                    }
                    let position = position as usize;
                    return Some((
                        node.keys[position],
                        node.children[position]
                            .as_deref()
                            .expect("Node16 槽位必须存在"),
                    ));
                }
                ChildIterKind::Node48(node) => {
                    let node = *node;
                    if !(0..=255).contains(&position) {
                        return None;
                    }
                    let edge = position as u8;
                    if let Some(slot) = node.slot(edge) {
                        return Some((
                            edge,
                            node.children[slot].as_deref().expect("Node48 槽位必须存在"),
                        ));
                    }
                }
                ChildIterKind::Node256(node) => {
                    let node = *node;
                    if !(0..=255).contains(&position) {
                        return None;
                    }
                    let edge = position as u8;
                    if let Some(child) = node.children[usize::from(edge)].as_deref() {
                        return Some((edge, child));
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
struct ArtNode<V> {
    prefix: Vec<u8>,
    terminal: Option<Terminal<V>>,
    children: Children<V>,
}

impl<V> ArtNode<V> {
    fn leaf(suffix: &[u8], key: Vec<u8>, value: V) -> Self {
        Self {
            prefix: suffix.to_vec(),
            terminal: Some(Terminal { key, value }),
            children: Children::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ArtMap<V> {
    root: Option<Box<ArtNode<V>>>,
    len: usize,
}

impl<V> Default for ArtMap<V> {
    fn default() -> Self {
        Self { root: None, len: 0 }
    }
}

impl<V> ArtMap<V> {
    #[cfg(test)]
    fn root_kind(&self) -> Option<u16> {
        self.root.as_deref().map(|root| match root.children {
            Children::Node4(_) => 4,
            Children::Node16(_) => 16,
            Children::Node48(_) => 48,
            Children::Node256(_) => 256,
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn insert(&mut self, key: Vec<u8>, value: V) -> Option<V> {
        let mut replaced = None;
        match self.root.as_mut() {
            Some(root) => insert_node(root, &key, 0, key.clone(), value, &mut replaced),
            None => self.root = Some(Box::new(ArtNode::leaf(&key, key.clone(), value))),
        }
        if replaced.is_none() {
            self.len += 1;
        }
        replaced
    }

    pub(crate) fn get(&self, key: &[u8]) -> Option<&V> {
        get_node(self.root.as_deref()?, key, 0)
    }

    pub(crate) fn prefix_entries(&self, prefix: &[u8]) -> Vec<(&[u8], &V)> {
        let mut output = Vec::new();
        if let Some(root) = self.root.as_deref() {
            collect_prefix_entries(root, prefix, 0, &mut output);
        }
        output
    }

    pub(crate) fn prefix_values(&self, prefix: &[u8]) -> Vec<&V> {
        let mut output = Vec::new();
        if let Some(root) = self.root.as_deref() {
            collect_prefix_values(root, prefix, 0, &mut output);
        }
        output
    }

    pub(crate) fn get_mut(&mut self, key: &[u8]) -> Option<&mut V> {
        get_mut_node(self.root.as_deref_mut()?, key, 0)
    }

    pub(crate) fn remove(&mut self, key: &[u8]) -> Option<V> {
        let root = self.root.as_mut()?;
        let removed = remove_node(root, key, 0);
        if removed.is_some() {
            self.len -= 1;
            if root.terminal.is_none() && root.children.is_empty() {
                self.root = None;
            }
        }
        removed
    }

    pub(crate) fn entries(&self, descending: bool) -> Vec<(&[u8], &V)> {
        let mut output = Vec::with_capacity(self.len);
        if let Some(root) = self.root.as_deref() {
            collect_all(root, descending, &mut output, usize::MAX);
        }
        output
    }

    pub(crate) fn ordered_values(&self, descending: bool, limit: usize) -> Vec<&V> {
        let mut output = Vec::with_capacity(limit.min(self.len));
        if let Some(root) = self.root.as_deref() {
            collect_all_values(root, descending, &mut output, limit);
        }
        output
    }

    pub(crate) fn range_values(
        &self,
        bound: &[u8],
        ordering: Ordering,
        inclusive: bool,
        descending: bool,
        limit: usize,
    ) -> Vec<&V> {
        let mut output = Vec::with_capacity(limit.min(self.len));
        if let Some(root) = self.root.as_deref() {
            collect_range_values(
                root,
                bound,
                ordering,
                inclusive,
                descending,
                &mut output,
                limit,
            );
        }
        output
    }
}

fn common_prefix(left: &[u8], right: &[u8]) -> usize {
    left.iter().zip(right).take_while(|(a, b)| a == b).count()
}

fn insert_node<V>(
    node: &mut ArtNode<V>,
    key: &[u8],
    offset: usize,
    full_key: Vec<u8>,
    value: V,
    replaced: &mut Option<V>,
) {
    let remaining = &key[offset..];
    let shared = common_prefix(&node.prefix, remaining);
    if shared < node.prefix.len() {
        let old_edge = node.prefix[shared];
        let old_suffix = node.prefix[shared + 1..].to_vec();
        let old = ArtNode {
            prefix: old_suffix,
            terminal: node.terminal.take(),
            children: std::mem::take(&mut node.children),
        };
        node.prefix.truncate(shared);
        node.children.insert(old_edge, old);
        if shared == remaining.len() {
            node.terminal = Some(Terminal {
                key: full_key,
                value,
            });
        } else {
            let new_edge = remaining[shared];
            node.children.insert(
                new_edge,
                ArtNode::leaf(&remaining[shared + 1..], full_key, value),
            );
        }
        return;
    }

    let next_offset = offset + shared;
    if next_offset == key.len() {
        *replaced = node
            .terminal
            .replace(Terminal {
                key: full_key,
                value,
            })
            .map(|terminal| terminal.value);
        return;
    }
    let edge = key[next_offset];
    if let Some(child) = node.children.get_mut(edge) {
        insert_node(child, key, next_offset + 1, full_key, value, replaced);
    } else {
        node.children.insert(
            edge,
            ArtNode::leaf(&key[next_offset + 1..], full_key, value),
        );
    }
}

fn get_node<'a, V>(node: &'a ArtNode<V>, key: &[u8], offset: usize) -> Option<&'a V> {
    let common = common_prefix(&node.prefix, key.get(offset..)?);
    if common != node.prefix.len() {
        return None;
    }
    let next_offset = offset + common;
    if next_offset == key.len() {
        return node.terminal.as_ref().map(|terminal| &terminal.value);
    }
    let edge = key[next_offset];
    get_node(node.children.get(edge)?, key, next_offset + 1)
}

fn collect_prefix_entries<'a, V>(
    node: &'a ArtNode<V>,
    prefix: &[u8],
    offset: usize,
    output: &mut Vec<(&'a [u8], &'a V)>,
) {
    let Some(remaining) = prefix.get(offset..) else {
        return;
    };
    let shared = common_prefix(&node.prefix, remaining);
    if shared == remaining.len() {
        collect_all(node, false, output, usize::MAX);
        return;
    }
    if shared != node.prefix.len() {
        return;
    }
    let next_offset = offset + shared;
    let edge = prefix[next_offset];
    if let Some(child) = node.children.get(edge) {
        collect_prefix_entries(child, prefix, next_offset + 1, output);
    }
}

fn collect_prefix_values<'a, V>(
    node: &'a ArtNode<V>,
    prefix: &[u8],
    offset: usize,
    output: &mut Vec<&'a V>,
) {
    let Some(remaining) = prefix.get(offset..) else {
        return;
    };
    let shared = common_prefix(&node.prefix, remaining);
    if shared == remaining.len() {
        collect_all_values(node, false, output, usize::MAX);
        return;
    }
    if shared != node.prefix.len() {
        return;
    }
    let next_offset = offset + shared;
    let edge = prefix[next_offset];
    if let Some(child) = node.children.get(edge) {
        collect_prefix_values(child, prefix, next_offset + 1, output);
    }
}

fn get_mut_node<'a, V>(node: &'a mut ArtNode<V>, key: &[u8], offset: usize) -> Option<&'a mut V> {
    if !key.get(offset..)?.starts_with(&node.prefix) {
        return None;
    }
    let next_offset = offset + node.prefix.len();
    if next_offset == key.len() {
        return node.terminal.as_mut().map(|terminal| &mut terminal.value);
    }
    let edge = *key.get(next_offset)?;
    get_mut_node(node.children.get_mut(edge)?, key, next_offset + 1)
}

fn remove_node<V>(node: &mut ArtNode<V>, key: &[u8], offset: usize) -> Option<V> {
    if !key.get(offset..)?.starts_with(&node.prefix) {
        return None;
    }
    let next_offset = offset + node.prefix.len();
    let removed = if next_offset == key.len() {
        node.terminal.take().map(|terminal| terminal.value)
    } else {
        let edge = *key.get(next_offset)?;
        let child = node.children.get_mut(edge)?;
        let removed = remove_node(child, key, next_offset + 1);
        if removed.is_some() && child.terminal.is_none() && child.children.is_empty() {
            node.children.remove(edge);
        }
        removed
    };
    if removed.is_some()
        && node.terminal.is_none()
        && node.children.len() == 1
        && let Some((edge, child)) = node.children.take_only_child()
    {
        let child = *child;
        node.prefix.push(edge);
        node.prefix.extend_from_slice(&child.prefix);
        node.terminal = child.terminal;
        node.children = child.children;
    }
    removed
}

fn collect_all<'a, V>(
    node: &'a ArtNode<V>,
    descending: bool,
    output: &mut Vec<(&'a [u8], &'a V)>,
    limit: usize,
) {
    if output.len() >= limit {
        return;
    }
    if !descending && let Some(terminal) = &node.terminal {
        output.push((&terminal.key, &terminal.value));
    }
    for (_, child) in node.children.iter(descending) {
        collect_all(child, descending, output, limit);
        if output.len() >= limit {
            break;
        }
    }
    if descending
        && output.len() < limit
        && let Some(terminal) = &node.terminal
    {
        output.push((&terminal.key, &terminal.value));
    }
}

fn collect_all_values<'a, V>(
    node: &'a ArtNode<V>,
    descending: bool,
    output: &mut Vec<&'a V>,
    limit: usize,
) {
    if output.len() >= limit {
        return;
    }
    if !descending && let Some(terminal) = &node.terminal {
        output.push(&terminal.value);
    }
    for (_, child) in node.children.iter(descending) {
        collect_all_values(child, descending, output, limit);
        if output.len() >= limit {
            break;
        }
    }
    if descending
        && output.len() < limit
        && let Some(terminal) = &node.terminal
    {
        output.push(&terminal.value);
    }
}

fn first_key<V>(node: &ArtNode<V>) -> Option<&[u8]> {
    node.terminal
        .as_ref()
        .map(|terminal| terminal.key.as_slice())
        .or_else(|| {
            node.children
                .first()
                .and_then(|(_, child)| first_key(child))
        })
}

fn last_key<V>(node: &ArtNode<V>) -> Option<&[u8]> {
    node.children
        .last()
        .and_then(|(_, child)| last_key(child))
        .or_else(|| {
            node.terminal
                .as_ref()
                .map(|terminal| terminal.key.as_slice())
        })
}

fn key_matches(key: &[u8], bound: &[u8], ordering: Ordering, inclusive: bool) -> bool {
    let comparison = key.cmp(bound);
    match ordering {
        Ordering::Greater => {
            comparison == Ordering::Greater || inclusive && comparison == Ordering::Equal
        }
        Ordering::Less => {
            comparison == Ordering::Less || inclusive && comparison == Ordering::Equal
        }
        Ordering::Equal => comparison == Ordering::Equal,
    }
}

fn subtree_relation<V>(
    node: &ArtNode<V>,
    bound: &[u8],
    ordering: Ordering,
    inclusive: bool,
) -> Option<bool> {
    let first = first_key(node)?;
    let last = last_key(node)?;
    if key_matches(first, bound, ordering, inclusive)
        && key_matches(last, bound, ordering, inclusive)
    {
        Some(true)
    } else if !key_matches(first, bound, ordering, inclusive)
        && !key_matches(last, bound, ordering, inclusive)
        && ordering != Ordering::Equal
    {
        Some(false)
    } else {
        None
    }
}

fn collect_range_values<'a, V>(
    node: &'a ArtNode<V>,
    bound: &[u8],
    ordering: Ordering,
    inclusive: bool,
    descending: bool,
    output: &mut Vec<&'a V>,
    limit: usize,
) {
    if output.len() >= limit {
        return;
    }
    match subtree_relation(node, bound, ordering, inclusive) {
        Some(true) => {
            collect_all_values(node, descending, output, limit);
            return;
        }
        Some(false) => return,
        None => {}
    }
    if !descending {
        if let Some(terminal) = &node.terminal
            && key_matches(&terminal.key, bound, ordering, inclusive)
        {
            output.push(&terminal.value);
        }
        for (_, child) in node.children.iter(false) {
            collect_range_values(child, bound, ordering, inclusive, descending, output, limit);
            if output.len() >= limit {
                break;
            }
        }
    } else {
        for (_, child) in node.children.iter(true) {
            collect_range_values(child, bound, ordering, inclusive, descending, output, limit);
            if output.len() >= limit {
                break;
            }
        }
        if output.len() < limit
            && let Some(terminal) = &node.terminal
            && key_matches(&terminal.key, bound, ordering, inclusive)
        {
            output.push(&terminal.value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ArtMap;
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};
    use std::cmp::Ordering;
    use std::collections::BTreeMap;

    #[test]
    fn art_前缀扫描与_btreemap_结果一致() {
        let mut art = ArtMap::default();
        let mut reference = BTreeMap::new();
        for (key, value) in [
            (b"ab".to_vec(), 1),
            (b"abc".to_vec(), 2),
            (b"abd".to_vec(), 3),
            (b"b".to_vec(), 4),
        ] {
            art.insert(key.clone(), value);
            reference.insert(key, value);
        }
        for prefix in [b"".as_slice(), b"a", b"ab", b"abc", b"abe", b"b"] {
            let actual = art
                .prefix_values(prefix)
                .into_iter()
                .copied()
                .collect::<Vec<_>>();
            let expected = reference
                .iter()
                .filter(|(key, _)| key.starts_with(prefix))
                .map(|(_, value)| *value)
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "prefix={prefix:?}");
        }
    }

    #[test]
    fn art_稠密节点位图跨越四个字边界() {
        let mut art = ArtMap::default();
        let boundary_edges = [
            0u8, 1, 62, 63, 64, 65, 126, 127, 128, 129, 190, 191, 192, 193, 254, 255,
        ];
        for byte in boundary_edges {
            art.insert(vec![byte], byte);
        }
        for byte in 10u8..44 {
            art.insert(vec![byte], byte);
        }
        let mut expected = boundary_edges
            .into_iter()
            .chain(10u8..44)
            .collect::<Vec<_>>();
        expected.sort_unstable();
        expected.dedup();
        assert_eq!(
            art.entries(false)
                .into_iter()
                .map(|(key, _)| key[0])
                .collect::<Vec<_>>(),
            expected
        );
        let mut descending = expected.clone();
        descending.reverse();
        assert_eq!(
            art.entries(true)
                .into_iter()
                .map(|(key, _)| key[0])
                .collect::<Vec<_>>(),
            descending
        );
        for byte in [0u8, 63, 64, 127, 128, 191, 192, 255] {
            assert_eq!(art.remove(&[byte]), Some(byte));
        }
        assert_eq!(
            art.entries(false)
                .into_iter()
                .map(|(key, _)| key[0])
                .collect::<Vec<_>>(),
            expected
                .into_iter()
                .filter(|byte| ![0u8, 63, 64, 127, 128, 191, 192, 255].contains(byte))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn art_真实节点布局跨越全部容量边界() {
        let mut art = ArtMap::default();
        for byte in 0u8..4 {
            art.insert(vec![byte], byte);
        }
        assert_eq!(art.root_kind(), Some(4));
        art.insert(vec![4], 4);
        assert_eq!(art.root_kind(), Some(16));
        for byte in 5u8..17 {
            art.insert(vec![byte], byte);
        }
        assert_eq!(art.root_kind(), Some(48));
        for byte in 17u8..49 {
            art.insert(vec![byte], byte);
        }
        assert_eq!(art.root_kind(), Some(256));

        assert_eq!(art.remove(&[48]), Some(48));
        assert_eq!(art.root_kind(), Some(48));
        for byte in (16u8..48).rev() {
            assert_eq!(art.remove(&[byte]), Some(byte));
        }
        assert_eq!(art.root_kind(), Some(16));
        for byte in (4u8..16).rev() {
            assert_eq!(art.remove(&[byte]), Some(byte));
        }
        assert_eq!(art.root_kind(), Some(4));
    }

    #[test]
    fn art_node48_删除空槽后重用映射保持有序() {
        let mut art = ArtMap::default();
        for byte in 0u8..40 {
            art.insert(vec![byte], u16::from(byte));
        }
        for byte in (1u8..40).step_by(2) {
            assert_eq!(art.remove(&[byte]), Some(u16::from(byte)));
        }
        for byte in 100u8..119 {
            art.insert(vec![byte], u16::from(byte));
        }
        let actual = art
            .entries(false)
            .into_iter()
            .map(|(key, value)| (key[0], *value))
            .collect::<Vec<_>>();
        let mut expected = (0u8..40)
            .filter(|byte| byte % 2 == 0)
            .chain(100u8..119)
            .map(|byte| (byte, u16::from(byte)))
            .collect::<Vec<_>>();
        expected.sort_unstable();
        assert_eq!(actual, expected);
        assert_eq!(art.root_kind(), Some(48));
    }

    #[test]
    fn art_节点自适应升级并在删除后收缩() {
        let mut art = ArtMap::default();
        for byte in 0u8..=200 {
            art.insert(vec![byte], byte);
        }
        assert_eq!(art.root_kind(), Some(256));
        for byte in 4u8..=200 {
            assert_eq!(art.remove(&[byte]), Some(byte));
        }
        assert_eq!(art.root_kind(), Some(4));
        assert_eq!(art.len(), 4);
    }

    #[test]
    fn art_路径压缩增删和有序遍历() {
        let mut art = ArtMap::default();
        for (index, key) in [b"abcd".as_slice(), b"abce", b"ab", b"xyz", b""]
            .into_iter()
            .enumerate()
        {
            art.insert(key.to_vec(), index);
        }
        assert_eq!(
            art.entries(false)
                .into_iter()
                .map(|(key, _)| key.to_vec())
                .collect::<Vec<_>>(),
            vec![
                b"".to_vec(),
                b"ab".to_vec(),
                b"abcd".to_vec(),
                b"abce".to_vec(),
                b"xyz".to_vec()
            ]
        );
        assert_eq!(art.remove(b"abcd"), Some(0));
        assert_eq!(art.remove(b"abce"), Some(1));
        assert_eq!(art.get_mut(b"ab"), Some(&mut 2));
        assert_eq!(art.len(), 3);
    }

    #[test]
    fn art_十万随机操作与_btreemap_差分一致() {
        let mut art = ArtMap::default();
        let mut truth = BTreeMap::new();
        let mut rng = StdRng::seed_from_u64(0xA2B0_1000);
        for operation in 0..100_000u32 {
            let key_len = rng.gen_range(0..=24);
            let key: Vec<u8> = (0..key_len).map(|_| rng.r#gen()).collect();
            match rng.gen_range(0..3) {
                0 | 1 => {
                    let value = rng.r#gen::<u64>();
                    assert_eq!(art.insert(key.clone(), value), truth.insert(key, value));
                }
                _ => assert_eq!(art.remove(&key), truth.remove(&key)),
            }
            if operation % 1_000 == 0 {
                assert_eq!(art.len(), truth.len());
                assert_eq!(
                    art.entries(false)
                        .into_iter()
                        .map(|(key, value)| (key.to_vec(), *value))
                        .collect::<Vec<_>>(),
                    truth
                        .iter()
                        .map(|(key, value)| (key.clone(), *value))
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn art_范围随机差分覆盖四种边界和双向顺序() {
        let mut art = ArtMap::default();
        let mut truth = BTreeMap::new();
        let mut rng = StdRng::seed_from_u64(0xA2B0_AA55);
        for _ in 0..10_000 {
            let key = rng.r#gen::<u64>().to_be_bytes().to_vec();
            let value = rng.r#gen::<u32>();
            art.insert(key.clone(), value);
            truth.insert(key, value);
        }
        for _ in 0..1_000 {
            let bound = rng.r#gen::<u64>().to_be_bytes().to_vec();
            for ordering in [std::cmp::Ordering::Less, std::cmp::Ordering::Greater] {
                for inclusive in [false, true] {
                    for descending in [false, true] {
                        let limit = rng.gen_range(1..=100);
                        let actual: Vec<_> = art
                            .range_values(&bound, ordering, inclusive, descending, limit)
                            .into_iter()
                            .copied()
                            .collect();
                        let mut expected: Vec<_> = truth
                            .iter()
                            .filter(|(key, _)| super::key_matches(key, &bound, ordering, inclusive))
                            .map(|(_, value)| *value)
                            .collect();
                        if descending {
                            expected.reverse();
                        }
                        expected.truncate(limit);
                        assert_eq!(actual, expected);
                    }
                }
            }
        }
    }

    #[test]
    fn art_范围和降序_limit() {
        let mut art = ArtMap::default();
        for value in 0u16..300 {
            art.insert(value.to_be_bytes().to_vec(), value);
        }
        assert_eq!(
            art.range_values(&100u16.to_be_bytes(), Ordering::Greater, false, false, 3)
                .into_iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![101, 102, 103]
        );
        assert_eq!(
            art.ordered_values(true, 3)
                .into_iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![299, 298, 297]
        );
    }
}
