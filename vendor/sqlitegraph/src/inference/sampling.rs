//! Sampling strategies for sparse inference.
//!
//! Supports temperature scaling and top-p (nucleus) sampling.

/// Sample next token from logits using temperature + top-p (nucleus) sampling.
///
/// # Arguments
/// * `logits` - Raw logits [vocab_size]
/// * `temperature` - Temperature for scaling (1.0 = normal, <1 = sharper, >1 = more random)
/// * `top_p` - Nucleus sampling threshold (0.9 = consider tokens covering 90% of probability mass)
///
/// # Returns
/// Token ID (index into logits array)
pub fn sample_token(logits: &[f32], temperature: f32, top_p: f32) -> usize {
    let vocab_size = logits.len();
    if vocab_size == 0 {
        return 0;
    }

    // Temperature scaling
    let scaled: Vec<f32> = if temperature > 0.001 {
        logits.iter().map(|&l| l / temperature).collect()
    } else {
        // Greedy: find argmax
        return logits
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(0);
    };

    // Softmax: exp(x - max) for numerical stability
    let max_val = scaled.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mut probs: Vec<f32> = scaled.iter().map(|&x| (x - max_val).exp()).collect();

    // Normalize
    let sum: f32 = probs.iter().sum();
    if sum > 0.0 {
        for p in probs.iter_mut() {
            *p /= sum;
        }
    } else {
        // Fallback: uniform
        let uniform = 1.0 / vocab_size as f32;
        for p in probs.iter_mut() {
            *p = uniform;
        }
    }

    // Top-p (nucleus) sampling
    if top_p < 1.0 {
        // Create index-probability pairs, sort descending
        let mut indexed: Vec<(usize, f32)> =
            probs.iter().enumerate().map(|(i, &p)| (i, p)).collect();
        indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Find cutoff
        let mut cumsum = 0.0f32;
        let mut cutoff_idx = indexed.len();
        for (i, &(_, p)) in indexed.iter().enumerate() {
            cumsum += p;
            if cumsum >= top_p {
                cutoff_idx = i + 1;
                break;
            }
        }

        // Zero out everything after cutoff
        for &(_, _p) in indexed.iter().skip(cutoff_idx) {
            // We'll rebuild probs below
        }

        // Rebuild probs with only top-p tokens
        let mut new_probs = vec![0.0f32; vocab_size];
        let new_sum: f32 = indexed[..cutoff_idx]
            .iter()
            .map(|&(i, p)| {
                new_probs[i] = p;
                p
            })
            .sum();

        if new_sum > 0.0 {
            for p in new_probs.iter_mut() {
                *p /= new_sum;
            }
        }
        probs = new_probs;
    }

    // Weighted random sampling
    let r: f32 = rand::random::<f32>();
    let mut cumsum = 0.0f32;
    for (i, &p) in probs.iter().enumerate() {
        cumsum += p;
        if cumsum >= r {
            return i;
        }
    }

    // Fallback: last token
    vocab_size - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_greedy_sampling() {
        let logits = vec![0.1, 0.1, 5.0, 0.1, 0.1];
        let token = sample_token(&logits, 0.0, 1.0);
        assert_eq!(token, 2); // index of max value
    }

    #[test]
    fn test_temperature_sampling() {
        let logits = vec![1.0, 1.0, 1.0, 1.0];
        // With high temperature, all tokens roughly equal probability
        let mut counts = vec![0usize; 4];
        for _ in 0..1000 {
            let token = sample_token(&logits, 1.0, 1.0);
            assert!(token < 4);
            counts[token] += 1;
        }
        // Each token should appear ~250 times (±100)
        for &c in &counts {
            assert!(c > 100, "Token count {} too low", c);
        }
    }

    #[test]
    fn test_top_p_sampling() {
        // One dominant token, rest small
        let logits = vec![10.0, 0.0, 0.0, 0.0, 0.0];
        let token = sample_token(&logits, 1.0, 0.5);
        assert_eq!(token, 0); // top_p=0.5, dominant token covers >50%
    }
}
