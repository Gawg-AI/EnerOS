//! Linear algebra utilities shared across EnerOS crates.
//!
//! Provides Gaussian elimination-based matrix inversion and linear system solving
//! for both real and complex-valued matrices, using `Vec<Vec<T>>` representation
//! to avoid heavy dependencies.

#![allow(clippy::needless_range_loop)]

use num_complex::Complex64;

/// Invert a real-valued matrix using Gaussian elimination with partial pivoting.
///
/// Returns `None` if the matrix is singular or non-square.
pub fn gauss_elimination_inverse(matrix: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = matrix.len();
    if n == 0 {
        return Some(Vec::new());
    }
    // Check square
    for row in matrix {
        if row.len() != n {
            return None;
        }
    }

    // Build augmented matrix [A | I]
    let mut aug = vec![vec![0.0; 2 * n]; n];
    for i in 0..n {
        for j in 0..n {
            aug[i][j] = matrix[i][j];
        }
        aug[i][n + i] = 1.0;
    }

    for col in 0..n {
        // Partial pivoting
        let mut max_val = aug[col][col].abs();
        let mut max_row = col;
        for row in (col + 1)..n {
            if aug[row][col].abs() > max_val {
                max_val = aug[row][col].abs();
                max_row = row;
            }
        }

        if max_val < 1e-12 {
            return None;
        }

        if max_row != col {
            aug.swap(col, max_row);
        }

        let pivot = aug[col][col];
        for j in 0..(2 * n) {
            aug[col][j] /= pivot;
        }

        for row in 0..n {
            if row != col {
                let factor = aug[row][col];
                for j in 0..(2 * n) {
                    aug[row][j] -= factor * aug[col][j];
                }
            }
        }
    }

    let mut inv = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            inv[i][j] = aug[i][n + j];
        }
    }

    Some(inv)
}

/// Solve a real-valued linear system Ax = b using Gaussian elimination with partial pivoting.
///
/// Returns `None` if the matrix is singular.
pub fn solve_linear_system(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    if n == 0 {
        return Some(Vec::new());
    }

    let mut aug = vec![vec![0.0; n + 1]; n];
    for i in 0..n {
        for j in 0..n {
            aug[i][j] = a[i][j];
        }
        aug[i][n] = b[i];
    }

    for col in 0..n {
        let mut max_val = aug[col][col].abs();
        let mut max_row = col;
        for row in (col + 1)..n {
            if aug[row][col].abs() > max_val {
                max_val = aug[row][col].abs();
                max_row = row;
            }
        }

        if max_val < 1e-12 {
            return None;
        }

        if max_row != col {
            aug.swap(col, max_row);
        }

        for row in (col + 1)..n {
            let factor = aug[row][col] / aug[col][col];
            for k in col..=n {
                aug[row][k] -= factor * aug[col][k];
            }
        }
    }

    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        x[i] = aug[i][n];
        for j in (i + 1)..n {
            x[i] -= aug[i][j] * x[j];
        }
        x[i] /= aug[i][i];
    }

    Some(x)
}

/// Invert a complex-valued matrix using Gaussian elimination with partial pivoting.
///
/// Returns `None` if the matrix is singular or non-square.
pub fn invert_complex_matrix(matrix: &[Vec<Complex64>]) -> Option<Vec<Vec<Complex64>>> {
    let n = matrix.len();
    if n == 0 {
        return Some(Vec::new());
    }
    for row in matrix {
        if row.len() != n {
            return None;
        }
    }

    let mut aug = vec![vec![Complex64::new(0.0, 0.0); 2 * n]; n];
    for i in 0..n {
        for j in 0..n {
            aug[i][j] = matrix[i][j];
        }
        aug[i][n + i] = Complex64::new(1.0, 0.0);
    }

    for col in 0..n {
        let mut max_val = aug[col][col].norm();
        let mut max_row = col;
        for row in (col + 1)..n {
            if aug[row][col].norm() > max_val {
                max_val = aug[row][col].norm();
                max_row = row;
            }
        }

        if max_val < 1e-12 {
            return None;
        }

        if max_row != col {
            aug.swap(col, max_row);
        }

        let pivot = aug[col][col];
        for j in 0..(2 * n) {
            aug[col][j] /= pivot;
        }

        for row in 0..n {
            if row != col {
                let factor = aug[row][col];
                for j in 0..(2 * n) {
                    aug[row][j] = aug[row][j] - factor * aug[col][j];
                }
            }
        }
    }

    let mut inv = vec![vec![Complex64::new(0.0, 0.0); n]; n];
    for i in 0..n {
        for j in 0..n {
            inv[i][j] = aug[i][n + j];
        }
    }

    Some(inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_matrix_inverse() {
        let identity = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
        ];
        let inv = gauss_elimination_inverse(&identity).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!((inv[i][j] - expected).abs() < 1e-10, "inv[{}][{}] = {}, expected {}", i, j, inv[i][j], expected);
            }
        }
    }

    #[test]
    fn test_2x2_inverse() {
        // [[4, 7], [2, 6]] -> inverse = [[0.6, -0.7], [-0.2, 0.4]]
        let matrix = vec![
            vec![4.0, 7.0],
            vec![2.0, 6.0],
        ];
        let inv = gauss_elimination_inverse(&matrix).unwrap();
        assert!((inv[0][0] - 0.6).abs() < 1e-10);
        assert!((inv[0][1] - (-0.7)).abs() < 1e-10);
        assert!((inv[1][0] - (-0.2)).abs() < 1e-10);
        assert!((inv[1][1] - 0.4).abs() < 1e-10);
    }

    #[test]
    fn test_singular_matrix_returns_none() {
        let singular = vec![
            vec![1.0, 2.0],
            vec![2.0, 4.0],
        ];
        assert!(gauss_elimination_inverse(&singular).is_none());
    }

    #[test]
    fn test_3x3_inverse() {
        let matrix = vec![
            vec![1.0, 2.0, 3.0],
            vec![0.0, 1.0, 4.0],
            vec![5.0, 6.0, 0.0],
        ];
        let inv = gauss_elimination_inverse(&matrix).unwrap();

        // Verify A * A^{-1} = I
        for i in 0..3 {
            for j in 0..3 {
                let mut dot = 0.0;
                for k in 0..3 {
                    dot += matrix[i][k] * inv[k][j];
                }
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!((dot - expected).abs() < 1e-9, "A*A^-1 at [{},{}] = {}, expected {}", i, j, dot, expected);
            }
        }
    }

    #[test]
    fn test_solve_linear_system_identity() {
        let a = vec![
            vec![1.0, 0.0],
            vec![0.0, 1.0],
        ];
        let b = vec![3.0, 5.0];
        let x = solve_linear_system(&a, &b).unwrap();
        assert!((x[0] - 3.0).abs() < 1e-10);
        assert!((x[1] - 5.0).abs() < 1e-10);
    }

    #[test]
    fn test_solve_linear_system_general() {
        let a = vec![
            vec![2.0, 1.0],
            vec![1.0, 3.0],
        ];
        let b = vec![5.0, 7.0];
        let x = solve_linear_system(&a, &b).unwrap();
        assert!((x[0] - 1.6).abs() < 1e-10);
        assert!((x[1] - 1.8).abs() < 1e-10);
    }

    #[test]
    fn test_solve_linear_system_singular() {
        let a = vec![
            vec![1.0, 2.0],
            vec![2.0, 4.0],
        ];
        let b = vec![5.0, 10.0];
        assert!(solve_linear_system(&a, &b).is_none());
    }

    #[test]
    fn test_invert_complex_identity() {
        let identity = vec![
            vec![Complex64::new(1.0, 0.0), Complex64::new(0.0, 0.0)],
            vec![Complex64::new(0.0, 0.0), Complex64::new(1.0, 0.0)],
        ];
        let inv = invert_complex_matrix(&identity).unwrap();
        assert!((inv[0][0] - Complex64::new(1.0, 0.0)).norm() < 1e-10);
        assert!((inv[1][1] - Complex64::new(1.0, 0.0)).norm() < 1e-10);
        assert!(inv[0][1].norm() < 1e-10);
        assert!(inv[1][0].norm() < 1e-10);
    }

    #[test]
    fn test_invert_complex_singular() {
        let singular = vec![
            vec![Complex64::new(1.0, 0.0), Complex64::new(2.0, 0.0)],
            vec![Complex64::new(2.0, 0.0), Complex64::new(4.0, 0.0)],
        ];
        assert!(invert_complex_matrix(&singular).is_none());
    }
}
