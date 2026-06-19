use eneros_core::ElementId;
use num_complex::Complex64;
use sprs::CsMat;
use std::collections::HashMap;

/// Y-Bus 导纳矩阵（稀疏行压缩存储）
///
/// 内部使用 `Vec<Vec<(usize, Complex64)>>` 作为可变行压缩存储：
/// 每行一个按列索引升序排列的 `(col_idx, value)` 列表。
/// 通过 `to_csr()` 可转换为不可变 `sprs::CsMat<Complex64>`（CSR 格式）。
#[derive(Clone, Debug)]
pub struct YBusMatrix {
    size: usize,
    // 行压缩稀疏存储：rows[i] 为第 i 行的非零元列表，按列索引升序排列
    rows: Vec<Vec<(usize, Complex64)>>,
    bus_map: HashMap<ElementId, usize>,
    base_mva: f64,
    branch_ratings_mva: HashMap<(usize, usize), f64>,
}

impl YBusMatrix {
    /// 创建一个新的空 Y-Bus 矩阵
    pub fn new(size: usize) -> Self {
        Self {
            size,
            rows: vec![Vec::new(); size],
            bus_map: HashMap::new(),
            base_mva: 1.0,
            branch_ratings_mva: HashMap::new(),
        }
    }

    /// 设置母线索引映射
    pub fn set_bus_map(&mut self, bus_map: HashMap<ElementId, usize>) {
        self.bus_map = bus_map;
    }

    pub fn set_base_mva(&mut self, base_mva: f64) {
        if base_mva.is_finite() && base_mva > 0.0 {
            self.base_mva = base_mva;
        }
    }

    pub fn base_mva(&self) -> f64 {
        self.base_mva
    }

    pub fn set_branch_rating_mva(&mut self, from_idx: usize, to_idx: usize, rating_mva: f64) {
        if from_idx < self.size && to_idx < self.size && rating_mva.is_finite() && rating_mva > 0.0
        {
            let key = ordered_pair(from_idx, to_idx);
            self.branch_ratings_mva.insert(key, rating_mva);
        }
    }

    pub fn branch_rating_mva(&self, from_idx: usize, to_idx: usize) -> Option<f64> {
        self.branch_ratings_mva
            .get(&ordered_pair(from_idx, to_idx))
            .copied()
    }

    /// 获取矩阵元素 (G, B)。越界或缺失返回 (0.0, 0.0)。
    pub fn get(&self, i: usize, j: usize) -> (f64, f64) {
        if i >= self.size {
            return (0.0, 0.0);
        }
        // 二分查找列 j
        let row = &self.rows[i];
        match row.binary_search_by_key(&j, |(c, _)| *c) {
            Ok(idx) => {
                let v = row[idx].1;
                (v.re, v.im)
            }
            Err(_) => (0.0, 0.0),
        }
    }

    /// 设置矩阵元素（覆盖已有值或插入新值）
    pub fn set(&mut self, i: usize, j: usize, g: f64, b: f64) {
        if i >= self.size || j >= self.size {
            return;
        }
        let value = Complex64::new(g, b);
        let row = &mut self.rows[i];
        match row.binary_search_by_key(&j, |(c, _)| *c) {
            Ok(idx) => row[idx].1 = value,
            Err(idx) => row.insert(idx, (j, value)),
        }
    }

    /// 累加矩阵元素
    pub fn add(&mut self, i: usize, j: usize, g: f64, b: f64) {
        if i >= self.size || j >= self.size {
            return;
        }
        let delta = Complex64::new(g, b);
        let row = &mut self.rows[i];
        match row.binary_search_by_key(&j, |(c, _)| *c) {
            Ok(idx) => row[idx].1 += delta,
            Err(idx) => row.insert(idx, (j, delta)),
        }
    }

    /// 获取矩阵维度
    pub fn size(&self) -> usize {
        self.size
    }

    /// 返回非零元数量
    pub fn nnz(&self) -> usize {
        self.rows.iter().map(|r| r.len()).sum()
    }

    /// 迭代第 i 行非零元，返回 (col_idx, G, B)。
    /// 越界时返回空迭代器。
    pub fn iter_row(&self, i: usize) -> impl Iterator<Item = (usize, f64, f64)> + '_ {
        let row: &[(usize, Complex64)] = self.rows.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
        row.iter().map(|&(col, v)| (col, v.re, v.im))
    }

    /// 转换为 CSR 稀疏矩阵视图（`sprs::CsMat<Complex64>`）
    pub fn to_csr(&self) -> CsMat<Complex64> {
        let mut trimat = sprs::TriMat::new((self.size, self.size));
        for (row_idx, row) in self.rows.iter().enumerate() {
            for &(col, v) in row {
                trimat.add_triplet(row_idx, col, v);
            }
        }
        trimat.to_csr()
    }

    /// 从旧稠密格式 `&[Vec<(f64, f64)>]` 迁移构造稀疏矩阵。
    /// 绝对值小于 1e-15 的元素视为零，不存储。
    pub fn from_dense(data: &[Vec<(f64, f64)>]) -> Self {
        let size = data.len();
        let mut matrix = Self::new(size);
        for (i, row) in data.iter().enumerate() {
            for (j, &(g, b)) in row.iter().enumerate() {
                if g.abs() > 1e-15 || b.abs() > 1e-15 {
                    matrix.set(i, j, g, b);
                }
            }
        }
        matrix
    }

    /// 从支路数据构建 Y-Bus 矩阵（含变压器变比）
    /// branches: (from, to, r, x, b, tap_ratio)
    /// tap_ratio = 1.0 为普通线路，非 1.0 为变压器
    pub fn from_branches(
        branches: &[(ElementId, ElementId, f64, f64, f64, f64)],
        bus_map: &HashMap<ElementId, usize>,
    ) -> Self {
        let size = bus_map.len();
        let mut matrix = Self::new(size);
        matrix.set_bus_map(bus_map.clone());

        for &(from, to, r, x, b, tap) in branches {
            if let (Some(&i), Some(&j)) = (bus_map.get(&from), bus_map.get(&to)) {
                let z_sq = r * r + x * x;
                if z_sq > 1e-10 {
                    let g = r / z_sq;
                    let b_line = -x / z_sq;
                    let b_charging = b / 2.0;

                    if (tap - 1.0).abs() < 1e-10 {
                        // 普通线路（tap = 1.0）
                        matrix.add(i, i, g, b_line + b_charging);
                        matrix.add(j, j, g, b_line + b_charging);
                        matrix.add(i, j, -g, -b_line);
                        matrix.add(j, i, -g, -b_line);
                    } else {
                        // 非标准变比变压器
                        // Y_ii += y / tap^2, Y_jj += y, Y_ij = Y_ji = -y / tap
                        let tap_sq = tap * tap;
                        matrix.add(i, i, g / tap_sq, (b_line + b_charging) / tap_sq);
                        matrix.add(j, j, g, b_line + b_charging);
                        matrix.add(i, j, -g / tap, -b_line / tap);
                        matrix.add(j, i, -g / tap, -b_line / tap);
                    }
                }
            }
        }

        matrix
    }

    /// 向母线对角元素添加并联导纳
    pub fn add_shunt(&mut self, bus_idx: usize, g: f64, b: f64) {
        self.add(bus_idx, bus_idx, g, b);
    }
}

fn ordered_pair(a: usize, b: usize) -> (usize, usize) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_get_basic() {
        let mut m = YBusMatrix::new(3);
        m.set(0, 0, 1.5, 2.5);
        m.set(1, 2, 0.5, -0.5);

        assert_eq!(m.get(0, 0), (1.5, 2.5));
        assert_eq!(m.get(1, 2), (0.5, -0.5));
        // 未设置的元素返回零
        assert_eq!(m.get(0, 1), (0.0, 0.0));
        assert_eq!(m.get(2, 2), (0.0, 0.0));
        // 越界返回零
        assert_eq!(m.get(5, 0), (0.0, 0.0));
        assert_eq!(m.get(0, 5), (0.0, 0.0));
    }

    #[test]
    fn test_add_accumulate() {
        let mut m = YBusMatrix::new(2);
        m.add(0, 0, 1.0, 1.0);
        m.add(0, 0, 0.5, 0.5);
        assert_eq!(m.get(0, 0), (1.5, 1.5));

        // 对未存在位置 add 应插入
        m.add(1, 1, 2.0, -1.0);
        assert_eq!(m.get(1, 1), (2.0, -1.0));
    }

    #[test]
    fn test_set_overwrite() {
        let mut m = YBusMatrix::new(2);
        m.set(0, 0, 1.0, 1.0);
        m.set(0, 0, 3.0, 4.0);
        assert_eq!(m.get(0, 0), (3.0, 4.0));
    }

    #[test]
    fn test_nnz() {
        let mut m = YBusMatrix::new(3);
        assert_eq!(m.nnz(), 0);
        m.set(0, 0, 1.0, 0.0);
        m.set(0, 1, 2.0, 0.0);
        m.set(2, 2, 3.0, 0.0);
        assert_eq!(m.nnz(), 3);
        // 覆盖不增加 nnz
        m.set(0, 0, 5.0, 0.0);
        assert_eq!(m.nnz(), 3);
    }

    #[test]
    fn test_iter_row() {
        let mut m = YBusMatrix::new(3);
        m.set(0, 0, 1.0, 0.0);
        m.set(0, 2, 3.0, 4.0);
        m.set(1, 1, 2.0, 0.0);

        let row0: Vec<(usize, f64, f64)> = m.iter_row(0).collect();
        assert_eq!(row0, vec![(0, 1.0, 0.0), (2, 3.0, 4.0)]);

        let row1: Vec<(usize, f64, f64)> = m.iter_row(1).collect();
        assert_eq!(row1, vec![(1, 2.0, 0.0)]);

        let row2: Vec<(usize, f64, f64)> = m.iter_row(2).collect();
        assert!(row2.is_empty());

        // 越界返回空迭代器
        let row5: Vec<(usize, f64, f64)> = m.iter_row(5).collect();
        assert!(row5.is_empty());
    }

    #[test]
    fn test_to_csr() {
        let mut m = YBusMatrix::new(3);
        m.set(0, 0, 1.0, 0.0);
        m.set(0, 1, 2.0, 0.0);
        m.set(1, 1, 3.0, 0.0);
        m.set(2, 0, 4.0, 0.0);
        m.set(2, 2, 5.0, 0.0);

        let csr = m.to_csr();
        assert_eq!(csr.rows(), 3);
        assert_eq!(csr.cols(), 3);
        assert_eq!(csr.nnz(), 5);

        // 验证元素
        assert!((csr.get(0, 0).unwrap() - Complex64::new(1.0, 0.0)).norm() < 1e-15);
        assert!((csr.get(0, 1).unwrap() - Complex64::new(2.0, 0.0)).norm() < 1e-15);
        assert!((csr.get(1, 1).unwrap() - Complex64::new(3.0, 0.0)).norm() < 1e-15);
        assert!((csr.get(2, 0).unwrap() - Complex64::new(4.0, 0.0)).norm() < 1e-15);
        assert!((csr.get(2, 2).unwrap() - Complex64::new(5.0, 0.0)).norm() < 1e-15);
        // 零元素
        assert!(csr.get(0, 2).is_none());
        assert!(csr.get(1, 0).is_none());
    }

    #[test]
    fn test_from_dense() {
        let dense = vec![
            vec![(1.5, 0.0), (0.0, 0.0), (2.0, 1.0)],
            vec![(0.0, 0.0), (3.0, 0.0), (0.0, 0.0)],
            vec![(0.0, 0.0), (0.0, 0.0), (0.0, 0.0)],
        ];
        let m = YBusMatrix::from_dense(&dense);
        assert_eq!(m.size(), 3);
        assert_eq!(m.nnz(), 3);
        assert_eq!(m.get(0, 0), (1.5, 0.0));
        assert_eq!(m.get(0, 2), (2.0, 1.0));
        assert_eq!(m.get(1, 1), (3.0, 0.0));
        assert_eq!(m.get(2, 2), (0.0, 0.0));
    }

    #[test]
    fn test_from_branches_equivalence() {
        // 构造简单 2 节点系统，验证稀疏与原逻辑等价
        let mut bus_map = HashMap::new();
        bus_map.insert(0u64, 0);
        bus_map.insert(1u64, 1);
        let branches = vec![(0u64, 1u64, 0.01, 0.1, 0.0, 1.0)];
        let m = YBusMatrix::from_branches(&branches, &bus_map);

        // 对角元素应非零，非对角元素应非零
        let (g00, _b00) = m.get(0, 0);
        assert!(g00.abs() > 1e-6);
        let (g01, b01) = m.get(0, 1);
        assert!(g01.abs() > 1e-6 || b01.abs() > 1e-6);
        // 对称性
        assert_eq!(m.get(0, 1), m.get(1, 0));
        assert_eq!(m.get(0, 0), m.get(1, 1));
    }

    /// 性能基准：构造 IEEE-118 规模矩阵（118×118，~180 非零元），
    /// 验证 to_csr() + 稀疏求解 < 100ms。
    #[test]
    fn test_perf_ieee118_scale() {
        use std::time::Instant;

        let n = 118;
        let mut ybus = YBusMatrix::new(n);

        // 构造约 180 条支路（每条支路贡献 4 个非零元 → ~720 nnz，
        // 加上对角自导纳，与 IEEE-118 量级相当）
        let num_branches = 180;
        for k in 0..num_branches {
            let i = k % n;
            let j = (k * 7 + 3) % n;
            if i == j {
                continue;
            }
            let r = 0.01 + (k as f64) * 0.001;
            let x = 0.1 + (k as f64) * 0.01;
            let z_sq = r * r + x * x;
            let g = r / z_sq;
            let b = -x / z_sq;
            ybus.add(i, i, g, b);
            ybus.add(j, j, g, b);
            ybus.add(i, j, -g, -b);
            ybus.add(j, i, -g, -b);
        }

        // 确保对角元素非零（避免奇异）
        for i in 0..n {
            ybus.add(i, i, 1.5, 0.0);
        }

        let nnz = ybus.nnz();
        assert!(nnz > 100, "非零元数量过少: {}", nnz);

        // 计时：to_csr 转换
        let t0 = Instant::now();
        let csr = ybus.to_csr();
        let csr_time = t0.elapsed();
        assert_eq!(csr.nnz(), nnz);

        // 计时：使用 eneros-linalg 进行稀疏 LU 分解求解
        // 构造右端向量（单位向量）
        let b_vec: Vec<Complex64> = (0..n)
            .map(|i| Complex64::new(if i == 0 { 1.5 } else { 0.0 }, 0.0))
            .collect();

        let sparse_mat = eneros_linalg::SparseMatrix::from_csmat(csr);

        let t1 = Instant::now();
        let lu = eneros_linalg::SparseLuFactorization::new(&sparse_mat)
            .expect("LU 分解失败");
        let _x = lu.solve(&b_vec).expect("求解失败");
        let solve_time = t1.elapsed();

        let total = csr_time + solve_time;
        eprintln!(
            "IEEE-118 规模: nnz={}, to_csr={:?}, LU求解={:?}, 总计={:?}",
            nnz, csr_time, solve_time, total
        );

        // 性能要求：总计 < 100ms
        assert!(
            total.as_millis() < 100,
            "性能不达标: 总计 {:?} >= 100ms",
            total
        );
    }
}
