//! 极小的内存 LRU：波形这类「读一次用一会儿」的数据，按容量淘汰最久没碰的。
//!
//! 用 `Vec` 而不是 `HashMap + 链表`：容量只有几十条，线性查找比维护链表和哈希表
//! 更不容易写错；条目数一多就换成真正的 LRU 实现，接口不变。

/// 最近使用的在前。
pub struct Lru<K, V> {
    capacity: usize,
    entries: Vec<(K, V)>,
}

impl<K: PartialEq, V> Lru<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: Vec::new(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 命中就把它挪到最前面（最近使用）。容量为 0 时永远命中不了。
    pub fn get(&mut self, key: &K) -> Option<&V> {
        let index = self.entries.iter().position(|(entry, _)| entry == key)?;
        let entry = self.entries.remove(index);
        self.entries.insert(0, entry);
        self.entries.first().map(|(_, value)| value)
    }

    /// 插入并把最久没用的挤出去。相同的键只保留一份。
    pub fn insert(&mut self, key: K, value: V) {
        if self.capacity == 0 {
            return;
        }
        if let Some(index) = self.entries.iter().position(|(entry, _)| *entry == key) {
            self.entries.remove(index);
        }
        self.entries.insert(0, (key, value));
        self.entries.truncate(self.capacity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 满容量后淘汰最久没用的() {
        let mut lru = Lru::new(2);
        lru.insert("a", 1);
        lru.insert("b", 2);
        assert_eq!(lru.get(&"a"), Some(&1), "a 变成最近使用");

        lru.insert("c", 3);
        assert_eq!(lru.len(), 2);
        assert_eq!(lru.get(&"b"), None, "b 最久没用，被挤出去");
        assert_eq!(lru.get(&"a"), Some(&1));
        assert_eq!(lru.get(&"c"), Some(&3));
    }

    #[test]
    fn 重复插入只保留一份() {
        let mut lru = Lru::new(3);
        lru.insert("a", 1);
        lru.insert("b", 2);
        lru.insert("a", 10);

        assert_eq!(lru.len(), 2);
        assert_eq!(lru.get(&"a"), Some(&10));
        assert_eq!(lru.get(&"b"), Some(&2));
    }

    #[test]
    fn 容量为零时什么都不记() {
        let mut lru = Lru::new(0);
        lru.insert("a", 1);
        assert_eq!(lru.len(), 0);
        assert_eq!(lru.get(&"a"), None);
    }
}
