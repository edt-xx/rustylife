/// Recursive Least Squares (RLS) / Kalman filter with 3 inputs and 1 output.
///
/// Model: y = bias + c1*x1 + c2*x2 + c3*x3
/// State: [bias, c1, c2, c3] — 4 coefficients
/// H matrix: [1, x1, x2, x3] — built from inputs each step
///
/// Usage:
///   let mut kf = KalmanFilter::new(process_noise, measurement_noise);
///   kf.predict();
///   kf.update(inputs, measurement);
///   let pred = kf.predict_output(inputs);

pub struct KalmanFilter {
    /// State vector: [bias, c1, c2, c3]
    x: [f64; 4],
    /// Covariance matrix P (4x4, row-major)
    p: [[f64; 4]; 4],
    /// Process noise (Q) — how much the model drifts between steps
    q: f64,
    /// Measurement noise (R) — how noisy the observations are
    r: f64,
    /// Number of updates performed (for convergence checks)
    update_count: u64,
}

impl KalmanFilter {
    /// Create a new filter.
    /// - `process_noise`: how much coefficients are expected to drift per step
    /// - `measurement_noise`: how noisy the measurements are
    pub fn new(process_noise: f64, measurement_noise: f64) -> Self {
        let mut p = [[0f64; 4]; 4];
        // Initialize P with large diagonal (high initial uncertainty)
        for i in 0..4 {
            p[i][i] = 1e6;
        }
        Self {
            x: [0.0; 4],
            p,
            q: process_noise,
            r: measurement_noise,
            update_count: 0,
        }
    }

    /// Predict step: propagate covariance with process noise.
    /// (State x is constant between steps in RLS — no dynamics model.)
    pub fn predict(&mut self) {
        // Add process noise to diagonal of P
        for i in 0..4 {
            self.p[i][i] += self.q;
        }
    }

    /// Update with measurement.
    /// - `inputs`: [x1, x2, x3] — the three input features
    /// - `measurement`: observed output value
    pub fn update(&mut self, inputs: [f64; 3], measurement: f64) {
        self.update_count += 1;
        let h = [1.0, inputs[0], inputs[1], inputs[2]]; // H row vector

        // Innovation: y - H*x
        let mut hx = 0.0;
        for i in 0..4 {
            hx += h[i] * self.x[i];
        }
        let innovation = measurement - hx;

        // Kalman gain: K = P * H^T / (H * P * H^T + R)
        let mut php = 0.0;
        let mut k = [0.0; 4];
        for i in 0..4 {
            // (P * H^T)[i] = sum_j P[i][j] * H[j]
            let mut ph = 0.0;
            for j in 0..4 {
                ph += self.p[i][j] * h[j];
            }
            k[i] = ph;
            php += h[i] * ph;
        }
        let denom = php + self.r;
        if denom.abs() < 1e-15 {
            return; // Avoid division by zero
        }
        for i in 0..4 {
            k[i] /= denom;
        }

        // Update state: x = x + K * innovation
        for i in 0..4 {
            self.x[i] += k[i] * innovation;
        }

        // Update covariance: P = (I - K*H) * P
        // Compute (I - K*H) first
        let mut ikh = [[0f64; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                ikh[i][j] = if i == j { 1.0 } else { 0.0 } - k[i] * h[j];
            }
        }
        // P_new = (I - K*H) * P
        let mut new_p = [[0f64; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                let mut s = 0.0;
                for l in 0..4 {
                    s += ikh[i][l] * self.p[l][j];
                }
                new_p[i][j] = s;
            }
        }
        self.p = new_p;
    }

    /// Predict output for given inputs (without updating state).
    pub fn predict_output(&self, inputs: [f64; 3]) -> f64 {
        let h = [1.0, inputs[0], inputs[1], inputs[2]];
        let mut result = 0.0;
        for i in 0..4 {
            result += h[i] * self.x[i];
        }
        result
    }

    /// Return current coefficient vector [bias, c1, c2, c3].
    pub fn coefficients(&self) -> [f64; 4] {
        self.x
    }

    /// Number of updates performed.
    pub fn update_count(&self) -> u64 {
        self.update_count
    }

    /// Find the value of input[1] (second input) that maximizes predicted output.
    /// Sweeps `input[1]` over `[start, end)` in steps of `step_size`, holding
    /// `input[0]` and `input[2]` fixed. Returns `(best_input_1, best_predicted_output)`.
    pub fn find_optimal_input1(&self, input0: f64, input2: f64, start: f64, end: f64, step_size: f64) -> (f64, f64) {
        let mut best_val = start;
        let mut best_output = f64::NEG_INFINITY;

        let mut v = start;
        while v < end {
            let pred = self.predict_output([input0, v, input2]);
            if pred > best_output {
                best_output = pred;
                best_val = v;
            }
            v += step_size;
        }
        (best_val, best_output)
    }
}
