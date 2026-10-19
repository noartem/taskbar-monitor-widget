//! Fixed-size sample history for graphs.

/// Ring buffer of samples, oldest first. `NaN` marks missing samples.
#[derive(Debug, Clone)]
pub struct History {
    items: Vec<f64>,
    start: usize,
    len: usize,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self { items: vec![f64::NAN; capacity.max(1)], start: 0, len: 0 }
    }

    pub fn capacity(&self) -> usize {
        self.items.len()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<f64> {
        (index < self.len).then(|| self.items[(self.start + index) % self.items.len()])
    }

    pub fn last(&self) -> Option<f64> {
        self.len.checked_sub(1).and_then(|i| self.get(i))
    }

    pub fn push(&mut self, value: f64) {
        let cap = self.items.len();
        if self.len < cap {
            self.items[(self.start + self.len) % cap] = value;
            self.len += 1;
        } else {
            self.items[self.start] = value;
            self.start = (self.start + 1) % cap;
        }
    }

    /// Changes capacity, keeping the newest samples.
    pub fn resize(&mut self, capacity: usize) {
        let capacity = capacity.max(1);
        if capacity == self.items.len() {
            return;
        }
        let keep = self.len.min(capacity);
        let mut items: Vec<f64> = (self.len - keep..self.len).filter_map(|i| self.get(i)).collect();
        items.resize(capacity, f64::NAN);
        self.items = items;
        self.start = 0;
        self.len = keep;
    }

    pub fn iter(&self) -> impl Iterator<Item = f64> + '_ {
        (0..self.len).filter_map(|i| self.get(i))
    }

    /// Largest finite sample, or 0.
    pub fn max(&self) -> f64 {
        self.iter().filter(|v| v.is_finite()).fold(0.0, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_resizes() {
        let mut h = History::new(3);
        for v in 1..=5 {
            h.push(v as f64);
        }
        assert_eq!(h.iter().collect::<Vec<_>>(), vec![3.0, 4.0, 5.0]);
        assert_eq!(h.max(), 5.0);
        h.resize(2);
        assert_eq!(h.iter().collect::<Vec<_>>(), vec![4.0, 5.0]);
        h.resize(4);
        h.push(6.0);
        assert_eq!(h.iter().collect::<Vec<_>>(), vec![4.0, 5.0, 6.0]);
        assert_eq!(h.last(), Some(6.0));
    }
}
