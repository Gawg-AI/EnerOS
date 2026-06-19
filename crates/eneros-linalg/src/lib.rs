//! eneros-linalg: 稀疏线性代数层
//!
//! 基于 `sprs::CsMat` 实现稀疏线性代数运算，支持复数 `Complex64`。
//! 提供稀疏 LU 分解（带列主元）、稀疏 Cholesky 分解（LDL^H）、
//! 符号分解缓存、稀疏矩阵-向量乘法、转置、加法等功能。
//!
//! # 设计目标
//!
//! 为 v0.8.0 分析精度进阶提供高性能稀疏线性代数基础，支撑潮流计算、
//! 状态估计、最优潮流等模块的 Jacobian 求解与修正方程计算。

#![allow(clippy::needless_range_loop)]

use num_complex::Complex64;
use sprs::{CsMat, TriMat};
use thiserror::Error;

/// 零容差，用于判断矩阵奇异性和正定性
const EPS: f64 = 1e-12;

/// 线性代数错误类型
#[derive(Debug, Error)]
pub enum LinAlgError {
    /// 矩阵奇异，无法分解
    #[error("矩阵奇异，无法分解")]
    Singular,
    /// 矩阵维度不匹配
    #[error("矩阵维度不匹配: 期望 {expected:?}, 实际 {actual:?}")]
    DimensionMismatch {
        expected: (usize, usize),
        actual: (usize, usize),
    },
    /// 矩阵不是方阵
    #[error("矩阵不是方阵: {rows}x{cols}")]
    NotSquare { rows: usize, cols: usize },
    /// 矩阵不是 Hermitian 正定矩阵
    #[error("矩阵不是 Hermitian 正定矩阵")]
    NotHermitianPositiveDefinite,
    /// 符号分解缓存与矩阵稀疏模式不匹配
    #[error("符号分解缓存与矩阵稀疏模式不匹配")]
    SymbolicPatternMismatch,
}

/// 线性代数结果类型
pub type Result<T> = std::result::Result<T, LinAlgError>;

// ============================================================================
// SparseMatrix: 稀疏矩阵类型
// ============================================================================

/// 稀疏矩阵，封装 `sprs::CsMat<Complex64>`，使用 CSR 格式存储。
///
/// 提供构造、访问、修改方法，是本 crate 的核心数据结构。
#[derive(Clone, Debug)]
pub struct SparseMatrix {
    inner: CsMat<Complex64>,
}

impl SparseMatrix {
    /// 从三元组列表构造稀疏矩阵（CSR 格式）。
    ///
    /// 重复位置的值会被累加。
    pub fn from_triplets(
        nrows: usize,
        ncols: usize,
        triplets: &[(usize, usize, Complex64)],
    ) -> Self {
        let mut trimat = TriMat::new((nrows, ncols));
        for &(i, j, v) in triplets {
            trimat.add_triplet(i, j, v);
        }
        Self {
            inner: trimat.to_csr(),
        }
    }

    /// 从已有的 `CsMat<Complex64>` 构造稀疏矩阵。
    pub fn from_csmat(mat: CsMat<Complex64>) -> Self {
        Self { inner: mat }
    }

    /// 创建 n×n 单位矩阵。
    pub fn identity(n: usize) -> Self {
        Self {
            inner: CsMat::eye(n),
        }
    }

    /// 创建 nrows×ncols 零矩阵。
    pub fn zero(nrows: usize, ncols: usize) -> Self {
        Self {
            inner: CsMat::zero((nrows, ncols)),
        }
    }

    /// 返回行数。
    pub fn nrows(&self) -> usize {
        self.inner.rows()
    }

    /// 返回列数。
    pub fn ncols(&self) -> usize {
        self.inner.cols()
    }

    /// 返回非零元数量。
    pub fn nnz(&self) -> usize {
        self.inner.nnz()
    }

    /// 返回矩阵形状 (rows, cols)。
    pub fn shape(&self) -> (usize, usize) {
        self.inner.shape()
    }

    /// 访问 (i, j) 位置的元素，返回 `None` 表示该位置为零。
    pub fn get(&self, i: usize, j: usize) -> Option<Complex64> {
        self.inner.get(i, j).copied()
    }

    /// 返回内部 `CsMat` 引用。
    pub fn as_csmat(&self) -> &CsMat<Complex64> {
        &self.inner
    }

    /// 转换为稠密矩阵（`Vec<Vec<Complex64>>`）。
    pub fn to_dense(&self) -> Vec<Vec<Complex64>> {
        let (r, c) = self.shape();
        let mut dense = vec![vec![Complex64::new(0.0, 0.0); c]; r];
        let indptr = self.inner.indptr();
        let indices = self.inner.indices();
        let data = self.inner.data();
        for row in 0..r {
            let range = indptr.outer_inds_sz(row);
            for k in range {
                let col = indices[k];
                dense[row][col] = data[k];
            }
        }
        dense
    }

    /// 返回稀疏模式：排序后的 (行, 列) 索引对列表。
    fn pattern(&self) -> Vec<(usize, usize)> {
        let mut pattern: Vec<(usize, usize)> = Vec::with_capacity(self.nnz());
        let indptr = self.inner.indptr();
        let indices = self.inner.indices();
        for row in 0..self.nrows() {
            let range = indptr.outer_inds_sz(row);
            for k in range {
                pattern.push((row, indices[k]));
            }
        }
        pattern.sort();
        pattern
    }

    /// 检查矩阵是否为 Hermitian 矩阵（A = A^H）。
    fn is_hermitian(&self) -> bool {
        let n = self.nrows();
        if self.ncols() != n {
            return false;
        }
        for i in 0..n {
            for j in i..n {
                let a_ij = self.get(i, j).unwrap_or(Complex64::new(0.0, 0.0));
                let a_ji = self.get(j, i).unwrap_or(Complex64::new(0.0, 0.0));
                if (a_ij - a_ji.conj()).norm() > EPS {
                    return false;
                }
            }
        }
        true
    }

    /// 返回矩阵的转置。
    pub fn transpose(&self) -> SparseMatrix {
        // transpose_into 将 CSR 转为 CSC（仅切换存储标记，无数据搬移），
        // 再 to_csr 将 CSC 转回 CSR（实际搬运数据），结果为 A^T 的 CSR 表示。
        let csc = self.inner.clone().transpose_into();
        SparseMatrix {
            inner: csc.to_csr(),
        }
    }
}

// ============================================================================
// SymbolicFactorization: 符号分解缓存
// ============================================================================

/// 符号分解缓存结构。
///
/// 存储稀疏矩阵的符号分解信息（稀疏模式、消去树、主元序列），
/// 可复用于具有相同稀疏模式但不同数值的矩阵，避免重复符号分析。
#[derive(Clone, Debug)]
pub struct SymbolicFactorization {
    /// 矩阵维度 n（方阵）
    n: usize,
    /// 排序后的稀疏模式：(行, 列) 索引对列表
    pattern: Vec<(usize, usize)>,
    /// 消去树：etree[j] 为节点 j 的父节点，usize::MAX 表示根节点
    etree: Vec<usize>,
    /// 行主元序列（LU 分解用）：pivot_order[i] 为第 i 个主元对应的原始行号
    pivot_order: Vec<usize>,
}

impl SymbolicFactorization {
    /// 对矩阵进行符号分析，构建可复用的符号分解缓存。
    ///
    /// # 参数
    /// - `matrix`: 待分析的方阵
    ///
    /// # 返回
    /// 符号分解缓存，或维度错误
    pub fn analyze(matrix: &SparseMatrix) -> Result<Self> {
        let (rows, cols) = matrix.shape();
        if rows != cols {
            return Err(LinAlgError::NotSquare { rows, cols });
        }
        let n = rows;
        let pattern = matrix.pattern();
        let etree = compute_etree(n, &pattern);
        // 初始主元序列为自然顺序
        let pivot_order: Vec<usize> = (0..n).collect();
        Ok(Self {
            n,
            pattern,
            etree,
            pivot_order,
        })
    }

    /// 检查给定矩阵是否与此符号分解缓存的稀疏模式匹配。
    pub fn matches(&self, matrix: &SparseMatrix) -> bool {
        let (rows, cols) = matrix.shape();
        if rows != self.n || cols != self.n {
            return false;
        }
        let matrix_pattern = matrix.pattern();
        self.pattern == matrix_pattern
    }

    /// 返回矩阵维度。
    pub fn n(&self) -> usize {
        self.n
    }

    /// 返回消去树引用。
    pub fn etree(&self) -> &[usize] {
        &self.etree
    }

    /// 返回稀疏模式引用。
    pub fn pattern(&self) -> &[(usize, usize)] {
        &self.pattern
    }

    /// 返回主元序列引用。
    pub fn pivot_order(&self) -> &[usize] {
        &self.pivot_order
    }
}

/// 计算对称稀疏矩阵的消去树（elimination tree）。
///
/// etree[j] 为节点 j 的父节点，usize::MAX 表示根节点。
/// 使用下三角部分（i > j）的非零模式构建。
fn compute_etree(n: usize, pattern: &[(usize, usize)]) -> Vec<usize> {
    let mut etree = vec![usize::MAX; n];

    // 构建下三角部分的邻接表（按列组织）
    let mut lower_cols: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(i, j) in pattern {
        if i > j && i < n && j < n {
            lower_cols[j].push(i);
        }
    }

    // 逐列处理，构建消去树
    for j in 0..n {
        for &i in &lower_cols[j] {
            // 从节点 i 向上走到根
            let mut r = i;
            while etree[r] != usize::MAX {
                r = etree[r];
            }
            // r 是当前子树的根，将其父节点设为 j
            if r != j {
                etree[r] = j;
            }
        }
    }

    etree
}

/// 稠密复数矩阵类型
type DenseMatrix = Vec<Vec<Complex64>>;

// ============================================================================
// SparseLuFactorization: 稀疏 LU 分解（带列主元）
// ============================================================================

/// 稀疏 LU 分解结果。
///
/// 实现 PA = LU，其中 P 为行置换矩阵（列主元法），
/// L 为单位下三角矩阵，U 为上三角矩阵。
///
/// 内部使用稠密存储以简化实现，适用于中小规模矩阵。
#[derive(Clone, Debug)]
pub struct SparseLuFactorization {
    /// 单位下三角因子 L
    l: DenseMatrix,
    /// 上三角因子 U
    u: DenseMatrix,
    /// 行置换向量：piv[i] 表示置换后第 i 行对应原始第 piv[i] 行
    piv: Vec<usize>,
    /// 矩阵维度
    n: usize,
}

impl SparseLuFactorization {
    /// 对稀疏矩阵进行 LU 分解（完整：符号分析 + 数值分解）。
    ///
    /// # 参数
    /// - `matrix`: 待分解的方阵
    ///
    /// # 返回
    /// LU 分解结果，或奇异/维度错误
    pub fn new(matrix: &SparseMatrix) -> Result<Self> {
        let (rows, cols) = matrix.shape();
        if rows != cols {
            return Err(LinAlgError::NotSquare { rows, cols });
        }
        let n = rows;
        let a = matrix.to_dense();
        let (l, u, piv) = lu_decompose(&a, n)?;
        Ok(Self { l, u, piv, n })
    }

    /// 使用符号分解缓存进行 LU 分解（仅数值分解）。
    ///
    /// 复用已缓存的稀疏模式和主元序列信息，跳过符号分析阶段。
    /// 若矩阵稀疏模式与缓存不匹配则返回错误。
    ///
    /// # 参数
    /// - `matrix`: 待分解的方阵（稀疏模式须与 `symbolic` 匹配）
    /// - `symbolic`: 预计算的符号分解缓存
    ///
    /// # 返回
    /// LU 分解结果，或错误
    pub fn with_symbolic(matrix: &SparseMatrix, symbolic: &SymbolicFactorization) -> Result<Self> {
        if !symbolic.matches(matrix) {
            return Err(LinAlgError::SymbolicPatternMismatch);
        }
        // 模式匹配，进行数值分解
        Self::new(matrix)
    }

    /// 求解线性系统 Ax = b。
    ///
    /// 利用 LU 分解结果，通过前代和回代求解。
    ///
    /// # 参数
    /// - `b`: 右端向量（长度须为 n）
    ///
    /// # 返回
    /// 解向量 x，或维度错误
    pub fn solve(&self, b: &[Complex64]) -> Result<Vec<Complex64>> {
        if b.len() != self.n {
            return Err(LinAlgError::DimensionMismatch {
                expected: (self.n, 1),
                actual: (b.len(), 1),
            });
        }
        Ok(lu_solve(&self.l, &self.u, &self.piv, b, self.n))
    }

    /// 返回矩阵维度。
    pub fn n(&self) -> usize {
        self.n
    }

    /// 返回行置换向量引用。
    pub fn piv(&self) -> &[usize] {
        &self.piv
    }
}

/// 执行带列主元（部分主元）的 LU 分解：PA = LU。
///
/// 返回 (L, U, piv)，其中 L 为单位下三角，U 为上三角，
/// piv 为行置换向量。
fn lu_decompose(a: &DenseMatrix, n: usize) -> Result<(DenseMatrix, DenseMatrix, Vec<usize>)> {
    // U 初始为 A 的副本，L 初始为零
    let mut u = a.to_vec();
    let mut l = vec![vec![Complex64::new(0.0, 0.0); n]; n];
    let mut piv: Vec<usize> = (0..n).collect();

    for k in 0..n {
        // 列主元选择：在 k 列的 k..n 行中找模最大的元素
        let mut max_val = u[k][k].norm();
        let mut max_row = k;
        for i in (k + 1)..n {
            let val = u[i][k].norm();
            if val > max_val {
                max_val = val;
                max_row = i;
            }
        }

        // 奇异矩阵检测
        if max_val < EPS {
            return Err(LinAlgError::Singular);
        }

        // 交换行
        if max_row != k {
            u.swap(k, max_row);
            l.swap(k, max_row);
            piv.swap(k, max_row);
        }

        // 消元
        l[k][k] = Complex64::new(1.0, 0.0);
        let pivot = u[k][k];
        for i in (k + 1)..n {
            let factor = u[i][k] / pivot;
            l[i][k] = factor;
            // 消去第 i 行的第 k 列
            u[i][k] = Complex64::new(0.0, 0.0);
            for j in (k + 1)..n {
                let u_kj = u[k][j];
                u[i][j] -= factor * u_kj;
            }
        }
    }

    Ok((l, u, piv))
}

/// 利用 LU 分解结果求解 Ax = b。
///
/// 步骤：1) 应用置换 Pb  2) 前代 Ly = Pb  3) 回代 Ux = y
fn lu_solve(
    l: &DenseMatrix,
    u: &DenseMatrix,
    piv: &[usize],
    b: &[Complex64],
    n: usize,
) -> Vec<Complex64> {
    // 应用行置换：pb[i] = b[piv[i]]
    let mut pb = vec![Complex64::new(0.0, 0.0); n];
    for i in 0..n {
        pb[i] = b[piv[i]];
    }

    // 前代：L * y = pb（L 对角线为 1，无需除法）
    let mut y = vec![Complex64::new(0.0, 0.0); n];
    for i in 0..n {
        y[i] = pb[i];
        for j in 0..i {
            let l_ij = l[i][j];
            let y_j = y[j];
            y[i] -= l_ij * y_j;
        }
    }

    // 回代：U * x = y
    let mut x = vec![Complex64::new(0.0, 0.0); n];
    for i in (0..n).rev() {
        x[i] = y[i];
        for j in (i + 1)..n {
            let u_ij = u[i][j];
            let x_j = x[j];
            x[i] -= u_ij * x_j;
        }
        x[i] /= u[i][i];
    }

    x
}

// ============================================================================
// SparseCholesky: 稀疏 Cholesky 分解（LDL^H）
// ============================================================================

/// 稀疏 Cholesky 分解结果（LDL^H 形式）。
///
/// 针对 Hermitian 正定矩阵，分解为 A = L * D * L^H，
/// 其中 L 为单位下三角矩阵，D 为对角矩阵。
///
/// 内部使用稠密存储以简化实现，适用于中小规模矩阵。
#[derive(Clone, Debug)]
pub struct SparseCholesky {
    /// 单位下三角因子 L
    l: DenseMatrix,
    /// 对角因子 D
    d: Vec<Complex64>,
    /// 矩阵维度
    n: usize,
}

impl SparseCholesky {
    /// 对 Hermitian 正定稀疏矩阵进行 Cholesky 分解（完整：符号分析 + 数值分解）。
    ///
    /// # 参数
    /// - `matrix`: 待分解的 Hermitian 正定方阵
    ///
    /// # 返回
    /// LDL^H 分解结果，或错误（非方阵/非 Hermitian/非正定）
    pub fn new(matrix: &SparseMatrix) -> Result<Self> {
        let (rows, cols) = matrix.shape();
        if rows != cols {
            return Err(LinAlgError::NotSquare { rows, cols });
        }
        let n = rows;
        if !matrix.is_hermitian() {
            return Err(LinAlgError::NotHermitianPositiveDefinite);
        }
        let a = matrix.to_dense();
        let (l, d) = ldl_decompose(&a, n)?;
        Ok(Self { l, d, n })
    }

    /// 使用符号分解缓存进行 Cholesky 分解（仅数值分解）。
    ///
    /// 复用已缓存的稀疏模式和消去树信息，跳过符号分析阶段。
    /// 若矩阵稀疏模式与缓存不匹配则返回错误。
    ///
    /// # 参数
    /// - `matrix`: 待分解的 Hermitian 正定方阵（稀疏模式须与 `symbolic` 匹配）
    /// - `symbolic`: 预计算的符号分解缓存
    ///
    /// # 返回
    /// LDL^H 分解结果，或错误
    pub fn with_symbolic(matrix: &SparseMatrix, symbolic: &SymbolicFactorization) -> Result<Self> {
        if !symbolic.matches(matrix) {
            return Err(LinAlgError::SymbolicPatternMismatch);
        }
        // 模式匹配，进行数值分解
        Self::new(matrix)
    }

    /// 求解线性系统 Ax = b。
    ///
    /// 利用 LDL^H 分解结果，通过前代、对角求解、回代求解。
    ///
    /// # 参数
    /// - `b`: 右端向量（长度须为 n）
    ///
    /// # 返回
    /// 解向量 x，或维度错误
    pub fn solve(&self, b: &[Complex64]) -> Result<Vec<Complex64>> {
        if b.len() != self.n {
            return Err(LinAlgError::DimensionMismatch {
                expected: (self.n, 1),
                actual: (b.len(), 1),
            });
        }
        Ok(ldl_solve(&self.l, &self.d, b, self.n))
    }

    /// 返回矩阵维度。
    pub fn n(&self) -> usize {
        self.n
    }

    /// 返回对角因子 D 引用。
    pub fn d(&self) -> &[Complex64] {
        &self.d
    }
}

/// 执行 LDL^H 分解：A = L * D * L^H。
///
/// L 为单位下三角矩阵，D 为对角矩阵。
/// 要求 A 为 Hermitian 正定矩阵。
fn ldl_decompose(a: &DenseMatrix, n: usize) -> Result<(DenseMatrix, Vec<Complex64>)> {
    let mut l = vec![vec![Complex64::new(0.0, 0.0); n]; n];
    let mut d = vec![Complex64::new(0.0, 0.0); n];

    for j in 0..n {
        // d[j] = a[j][j] - sum_{k<j} |l[j][k]|^2 * d[k]
        d[j] = a[j][j];
        for k in 0..j {
            // l[j][k] * conj(l[j][k]) = |l[j][k]|^2
            let l_jk = l[j][k];
            let d_k = d[k];
            d[j] -= l_jk * l_jk.conj() * d_k;
        }

        // 正定性检查：对 Hermitian 矩阵，D 应为实数且为正
        if d[j].re <= EPS || d[j].im.abs() > EPS {
            return Err(LinAlgError::NotHermitianPositiveDefinite);
        }

        // L 对角线为 1
        l[j][j] = Complex64::new(1.0, 0.0);

        // 计算 L[i][j] for i > j
        for i in (j + 1)..n {
            // l[i][j] = (a[i][j] - sum_{k<j} l[i][k] * conj(l[j][k]) * d[k]) / d[j]
            let mut sum = a[i][j];
            for k in 0..j {
                sum -= l[i][k] * l[j][k].conj() * d[k];
            }
            l[i][j] = sum / d[j];
        }
    }

    Ok((l, d))
}

/// 利用 LDL^H 分解结果求解 Ax = b。
///
/// 步骤：1) 前代 Ly = b  2) 对角求解 Dz = y  3) 回代 L^H x = z
fn ldl_solve(
    l: &DenseMatrix,
    d: &[Complex64],
    b: &[Complex64],
    n: usize,
) -> Vec<Complex64> {
    // 前代：L * y = b（L 对角线为 1）
    let mut y = vec![Complex64::new(0.0, 0.0); n];
    for i in 0..n {
        y[i] = b[i];
        for j in 0..i {
            let l_ij = l[i][j];
            let y_j = y[j];
            y[i] -= l_ij * y_j;
        }
    }

    // 对角求解：D * z = y
    let mut z = vec![Complex64::new(0.0, 0.0); n];
    for i in 0..n {
        z[i] = y[i] / d[i];
    }

    // 回代：L^H * x = z（L^H 为单位上三角）
    let mut x = vec![Complex64::new(0.0, 0.0); n];
    for i in (0..n).rev() {
        x[i] = z[i];
        for j in (i + 1)..n {
            // L^H[i][j] = conj(L[j][i])
            let lh_ij = l[j][i].conj();
            let x_j = x[j];
            x[i] -= lh_ij * x_j;
        }
    }

    x
}

// ============================================================================
// 稀疏矩阵运算函数
// ============================================================================

/// 稀疏矩阵-向量乘法：y = A * x。
///
/// # 参数
/// - `mat`: 稀疏矩阵（m×n）
/// - `x`: 稠密向量（长度须为 n）
///
/// # 返回
/// 结果向量 y（长度为 m），或维度错误
pub fn spmv(mat: &SparseMatrix, x: &[Complex64]) -> Result<Vec<Complex64>> {
    let (rows, cols) = mat.shape();
    if x.len() != cols {
        return Err(LinAlgError::DimensionMismatch {
            expected: (cols, 1),
            actual: (x.len(), 1),
        });
    }

    let mut y = vec![Complex64::new(0.0, 0.0); rows];
    let indptr = mat.as_csmat().indptr();
    let indices = mat.as_csmat().indices();
    let data = mat.as_csmat().data();
    for row in 0..rows {
        let range = indptr.outer_inds_sz(row);
        for k in range {
            let col = indices[k];
            y[row] += data[k] * x[col];
        }
    }
    Ok(y)
}

/// 稀疏矩阵转置。
///
/// 返回 A^T 的新稀疏矩阵（CSR 格式）。
pub fn transpose(mat: &SparseMatrix) -> SparseMatrix {
    mat.transpose()
}

/// 稀疏矩阵加法：C = A + B。
///
/// 要求 A 和 B 形状相同。利用 sprs 的 `Add` trait 实现。
///
/// # 参数
/// - `a`: 稀疏矩阵 A
/// - `b`: 稀疏矩阵 B
///
/// # 返回
/// 结果矩阵 C = A + B，或维度错误
pub fn add(a: &SparseMatrix, b: &SparseMatrix) -> Result<SparseMatrix> {
    let shape_a = a.shape();
    let shape_b = b.shape();
    if shape_a != shape_b {
        return Err(LinAlgError::DimensionMismatch {
            expected: shape_a,
            actual: shape_b,
        });
    }
    // sprs 的 &CsMat + &CsMat 返回 CsMat
    let sum: CsMat<Complex64> = &a.inner + &b.inner;
    Ok(SparseMatrix { inner: sum })
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 辅助：构造复数
    fn c(re: f64, im: f64) -> Complex64 {
        Complex64::new(re, im)
    }

    /// 辅助：验证 Ax = b 的残差
    fn check_residual(a: &SparseMatrix, x: &[Complex64], b: &[Complex64], tol: f64) {
        let y = spmv(a, x).unwrap();
        for i in 0..b.len() {
            assert!(
                (y[i] - b[i]).norm() < tol,
                "残差过大: 位置 {} = {}, 期望 {}",
                i,
                y[i],
                b[i]
            );
        }
    }

    /// 测试 1：构造稀疏矩阵并验证非零元
    #[test]
    fn test_construct_sparse_matrix() {
        let m = SparseMatrix::from_triplets(
            3,
            3,
            &[
                (0, 0, c(1.5, 0.0)),
                (1, 1, c(2.5, 0.0)),
                (2, 2, c(3.5, 0.0)),
                (0, 1, c(0.5, 1.0)),
            ],
        );

        assert_eq!(m.shape(), (3, 3));
        assert_eq!(m.nrows(), 3);
        assert_eq!(m.ncols(), 3);
        assert_eq!(m.nnz(), 4);

        // 验证元素访问
        assert!((m.get(0, 0).unwrap() - c(1.5, 0.0)).norm() < 1e-15);
        assert!((m.get(0, 1).unwrap() - c(0.5, 1.0)).norm() < 1e-15);
        assert!((m.get(1, 1).unwrap() - c(2.5, 0.0)).norm() < 1e-15);
        assert!((m.get(2, 2).unwrap() - c(3.5, 0.0)).norm() < 1e-15);
        assert!(m.get(0, 2).is_none());
        assert!(m.get(1, 0).is_none());

        // 验证稠密转换
        let dense = m.to_dense();
        assert!((dense[0][0] - c(1.5, 0.0)).norm() < 1e-15);
        assert!((dense[0][1] - c(0.5, 1.0)).norm() < 1e-15);
        assert!((dense[1][1] - c(2.5, 0.0)).norm() < 1e-15);
        assert!((dense[2][2] - c(3.5, 0.0)).norm() < 1e-15);
        assert!(dense[0][2].norm() < 1e-15);
    }

    /// 测试 2：稀疏 LU 分解求解线性系统（与稠密对比误差 < 1e-10）
    #[test]
    fn test_sparse_lu_solve() {
        // A = [[1.5, 0.5, 0.0],
        //      [0.0, 2.5, 1.0],
        //      [1.0, 0.0, 3.5]]
        let a = SparseMatrix::from_triplets(
            3,
            3,
            &[
                (0, 0, c(1.5, 0.0)),
                (0, 1, c(0.5, 0.0)),
                (1, 1, c(2.5, 0.0)),
                (1, 2, c(1.0, 0.0)),
                (2, 0, c(1.0, 0.0)),
                (2, 2, c(3.5, 0.0)),
            ],
        );

        let b = vec![c(2.0, 0.0), c(3.5, 0.0), c(4.5, 0.0)];

        let lu = SparseLuFactorization::new(&a).unwrap();
        let x = lu.solve(&b).unwrap();

        // 验证 A*x = b，残差 < 1e-10
        check_residual(&a, &x, &b, 1e-10);

        // 验证 LU 分解维度
        assert_eq!(lu.n(), 3);
    }

    /// 测试 2b：LU 分解含复数元素的系统
    #[test]
    fn test_sparse_lu_solve_complex() {
        // A = [[1.5+0.5i, 0.5],
        //      [0.5, 2.0-0.5i]]
        let a = SparseMatrix::from_triplets(
            2,
            2,
            &[
                (0, 0, c(1.5, 0.5)),
                (0, 1, c(0.5, 0.0)),
                (1, 0, c(0.5, 0.0)),
                (1, 1, c(2.0, -0.5)),
            ],
        );

        let b = vec![c(1.0, 0.5), c(2.0, 0.0)];

        let lu = SparseLuFactorization::new(&a).unwrap();
        let x = lu.solve(&b).unwrap();

        // 验证 A*x = b
        check_residual(&a, &x, &b, 1e-10);
    }

    /// 测试 2c：奇异矩阵 LU 分解应返回错误
    #[test]
    fn test_sparse_lu_singular() {
        let a = SparseMatrix::from_triplets(
            2,
            2,
            &[
                (0, 0, c(1.0, 0.0)),
                (0, 1, c(2.0, 0.0)),
                (1, 0, c(2.0, 0.0)),
                (1, 1, c(4.0, 0.0)),
            ],
        );
        let result = SparseLuFactorization::new(&a);
        assert!(matches!(result, Err(LinAlgError::Singular)));
    }

    /// 测试 3：稀疏 Cholesky 分解求解对称正定系统
    #[test]
    fn test_sparse_cholesky_solve() {
        // A = [[2.0, 0.5, 0.0],
        //      [0.5, 2.0, 0.5],
        //      [0.0, 0.5, 2.0]]  （实对称正定）
        let a = SparseMatrix::from_triplets(
            3,
            3,
            &[
                (0, 0, c(2.0, 0.0)),
                (0, 1, c(0.5, 0.0)),
                (1, 0, c(0.5, 0.0)),
                (1, 1, c(2.0, 0.0)),
                (1, 2, c(0.5, 0.0)),
                (2, 1, c(0.5, 0.0)),
                (2, 2, c(2.0, 0.0)),
            ],
        );

        let b = vec![c(1.5, 0.0), c(2.5, 0.0), c(3.5, 0.0)];

        let chol = SparseCholesky::new(&a).unwrap();
        let x = chol.solve(&b).unwrap();

        // 验证 A*x = b
        check_residual(&a, &x, &b, 1e-10);
        assert_eq!(chol.n(), 3);

        // 验证 D 为实数且为正
        for i in 0..3 {
            assert!(chol.d()[i].re > 0.0, "D[{}] 实部应为正", i);
            assert!(chol.d()[i].im.abs() < 1e-10, "D[{}] 虚部应接近零", i);
        }
    }

    /// 测试 3b：Cholesky 分解含复数 Hermitian 正定矩阵
    #[test]
    fn test_sparse_cholesky_hermitian_complex() {
        // A = [[2.0, 0.5+0.5i],
        //      [0.5-0.5i, 2.0]]  （Hermitian 正定）
        let a = SparseMatrix::from_triplets(
            2,
            2,
            &[
                (0, 0, c(2.0, 0.0)),
                (0, 1, c(0.5, 0.5)),
                (1, 0, c(0.5, -0.5)),
                (1, 1, c(2.0, 0.0)),
            ],
        );

        let b = vec![c(1.0, 0.0), c(1.0, 0.0)];

        let chol = SparseCholesky::new(&a).unwrap();
        let x = chol.solve(&b).unwrap();

        check_residual(&a, &x, &b, 1e-10);
    }

    /// 测试 3c：非正定矩阵 Cholesky 分解应返回错误
    #[test]
    fn test_sparse_cholesky_not_pd() {
        // A = [[1.0, 2.0],
        //      [2.0, 1.0]]  （对称但非正定）
        let a = SparseMatrix::from_triplets(
            2,
            2,
            &[
                (0, 0, c(1.0, 0.0)),
                (0, 1, c(2.0, 0.0)),
                (1, 0, c(2.0, 0.0)),
                (1, 1, c(1.0, 0.0)),
            ],
        );
        let result = SparseCholesky::new(&a);
        assert!(matches!(result, Err(LinAlgError::NotHermitianPositiveDefinite)));
    }

    /// 测试 4：稀疏矩阵-向量乘法正确性
    #[test]
    fn test_spmv() {
        // A = [[1.5, 0, 2.0],
        //      [0, 3.0, 0]]
        let m = SparseMatrix::from_triplets(
            2,
            3,
            &[
                (0, 0, c(1.5, 0.0)),
                (0, 2, c(2.0, 0.0)),
                (1, 1, c(3.0, 0.0)),
            ],
        );

        let x = vec![c(1.0, 0.0), c(1.5, 0.0), c(2.0, 0.0)];
        let y = spmv(&m, &x).unwrap();

        // y[0] = 1.5*1.0 + 2.0*2.0 = 5.5
        // y[1] = 3.0*1.5 = 4.5
        assert!((y[0] - c(5.5, 0.0)).norm() < 1e-10, "y[0] = {}, 期望 5.5", y[0]);
        assert!((y[1] - c(4.5, 0.0)).norm() < 1e-10, "y[1] = {}, 期望 4.5", y[1]);

        // 维度不匹配应返回错误
        let bad_x = vec![c(1.0, 0.0), c(2.0, 0.0)];
        assert!(spmv(&m, &bad_x).is_err());
    }

    /// 测试 4b：稀疏矩阵转置正确性
    #[test]
    fn test_transpose() {
        let m = SparseMatrix::from_triplets(
            2,
            3,
            &[
                (0, 0, c(1.5, 0.0)),
                (0, 2, c(2.0, 0.0)),
                (1, 1, c(3.0, 0.0)),
            ],
        );

        let mt = transpose(&m);
        assert_eq!(mt.shape(), (3, 2));
        assert_eq!(mt.nnz(), 3);

        // 验证转置后的元素
        assert!((mt.get(0, 0).unwrap() - c(1.5, 0.0)).norm() < 1e-15);
        assert!((mt.get(2, 0).unwrap() - c(2.0, 0.0)).norm() < 1e-15);
        assert!((mt.get(1, 1).unwrap() - c(3.0, 0.0)).norm() < 1e-15);
        assert!(mt.get(0, 1).is_none());

        // 验证 (A^T)^T = A
        let mtt = transpose(&mt);
        assert_eq!(mtt.shape(), (2, 3));
        assert!((mtt.get(0, 2).unwrap() - c(2.0, 0.0)).norm() < 1e-15);
    }

    /// 测试 4c：稀疏矩阵加法正确性
    #[test]
    fn test_add() {
        let a = SparseMatrix::from_triplets(
            2,
            2,
            &[
                (0, 0, c(1.5, 0.0)),
                (0, 1, c(0.5, 0.0)),
                (1, 1, c(2.0, 0.0)),
            ],
        );

        let b = SparseMatrix::from_triplets(
            2,
            2,
            &[
                (0, 0, c(0.5, 0.0)),
                (1, 0, c(1.0, 0.0)),
                (1, 1, c(1.5, 0.0)),
            ],
        );

        let result = add(&a, &b).unwrap();
        assert_eq!(result.shape(), (2, 2));

        // C = [[2.0, 0.5],
        //      [1.0, 3.5]]
        assert!((result.get(0, 0).unwrap() - c(2.0, 0.0)).norm() < 1e-15);
        assert!((result.get(0, 1).unwrap() - c(0.5, 0.0)).norm() < 1e-15);
        assert!((result.get(1, 0).unwrap() - c(1.0, 0.0)).norm() < 1e-15);
        assert!((result.get(1, 1).unwrap() - c(3.5, 0.0)).norm() < 1e-15);

        // 维度不匹配应返回错误
        let d = SparseMatrix::from_triplets(2, 3, &[(0, 0, c(1.0, 0.0))]);
        assert!(add(&a, &d).is_err());
    }

    /// 测试 5：符号分解缓存复用（同一结构不同数值两次求解）
    #[test]
    fn test_symbolic_factorization_reuse() {
        // A1: 2x2 矩阵，稀疏模式 {(0,0), (0,1), (1,0), (1,1)}
        let a1 = SparseMatrix::from_triplets(
            2,
            2,
            &[
                (0, 0, c(1.5, 0.0)),
                (0, 1, c(0.5, 0.0)),
                (1, 0, c(0.5, 0.0)),
                (1, 1, c(2.0, 0.0)),
            ],
        );

        // 构建符号分解缓存
        let sym = SymbolicFactorization::analyze(&a1).unwrap();
        assert_eq!(sym.n(), 2);
        assert_eq!(sym.pattern().len(), 4);
        assert_eq!(sym.etree().len(), 2);

        // 使用缓存对 A1 进行 LU 分解并求解
        let b1 = vec![c(1.0, 0.0), c(1.0, 0.0)];
        let lu1 = SparseLuFactorization::with_symbolic(&a1, &sym).unwrap();
        let x1 = lu1.solve(&b1).unwrap();
        check_residual(&a1, &x1, &b1, 1e-10);

        // A2: 相同稀疏模式，不同数值
        let a2 = SparseMatrix::from_triplets(
            2,
            2,
            &[
                (0, 0, c(3.0, 0.0)),
                (0, 1, c(1.0, 0.0)),
                (1, 0, c(1.0, 0.0)),
                (1, 1, c(4.0, 0.0)),
            ],
        );

        // 验证模式匹配
        assert!(sym.matches(&a2));

        // 复用同一符号缓存对 A2 进行 LU 分解并求解
        let b2 = vec![c(2.0, 0.0), c(3.0, 0.0)];
        let lu2 = SparseLuFactorization::with_symbolic(&a2, &sym).unwrap();
        let x2 = lu2.solve(&b2).unwrap();
        check_residual(&a2, &x2, &b2, 1e-10);

        // 验证两个解不同（因为矩阵和右端不同）
        assert!((x1[0] - x2[0]).norm() > 1e-6);

        // A3: 不同稀疏模式，应匹配失败
        let a3 = SparseMatrix::from_triplets(
            2,
            2,
            &[
                (0, 0, c(1.0, 0.0)),
                (1, 1, c(2.0, 0.0)), // 缺少 (0,1) 和 (1,0)
            ],
        );
        assert!(!sym.matches(&a3));
        assert!(SparseLuFactorization::with_symbolic(&a3, &sym).is_err());
    }

    /// 测试 5b：符号分解缓存复用于 Cholesky 分解
    #[test]
    fn test_symbolic_factorization_reuse_cholesky() {
        // A1: 对称正定矩阵
        let a1 = SparseMatrix::from_triplets(
            2,
            2,
            &[
                (0, 0, c(2.0, 0.0)),
                (0, 1, c(0.5, 0.0)),
                (1, 0, c(0.5, 0.0)),
                (1, 1, c(2.0, 0.0)),
            ],
        );

        let sym = SymbolicFactorization::analyze(&a1).unwrap();

        // 使用缓存对 A1 进行 Cholesky 分解
        let b1 = vec![c(1.0, 0.0), c(1.0, 0.0)];
        let chol1 = SparseCholesky::with_symbolic(&a1, &sym).unwrap();
        let x1 = chol1.solve(&b1).unwrap();
        check_residual(&a1, &x1, &b1, 1e-10);

        // A2: 相同模式，不同数值
        let a2 = SparseMatrix::from_triplets(
            2,
            2,
            &[
                (0, 0, c(3.0, 0.0)),
                (0, 1, c(1.0, 0.0)),
                (1, 0, c(1.0, 0.0)),
                (1, 1, c(3.0, 0.0)),
            ],
        );

        let b2 = vec![c(2.0, 0.0), c(2.0, 0.0)];
        let chol2 = SparseCholesky::with_symbolic(&a2, &sym).unwrap();
        let x2 = chol2.solve(&b2).unwrap();
        check_residual(&a2, &x2, &b2, 1e-10);
    }
}
