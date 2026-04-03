use std::sync::{OnceLock, RwLock};
use std::collections::HashMap;
use regex::Regex;

#[derive(Debug, Clone, Copy)]
pub struct CostPricing {
    pub input_cost_per_mtoken: f64,
    pub output_cost_per_mtoken: f64,
}

struct PricingRule {
    pattern: Regex,
    costs: CostPricing,
}

impl PricingRule {
    fn new(pattern: &str, input_cost: f64, output_cost: f64) -> Self {
        // Automatically make the regex case-insensitive
        let clean_pattern = pattern.replace("(?i)", "");
        let case_insensitive_pattern = format!("(?i){}", clean_pattern);
        Self {
            pattern: Regex::new(&case_insensitive_pattern).expect("Invalid model pricing regex"),
            costs: CostPricing {
                input_cost_per_mtoken: input_cost,
                output_cost_per_mtoken: output_cost,
            },
        }
    }
}

pub fn get_model_pricing(model: &str) -> CostPricing {
    static RULES: OnceLock<Vec<PricingRule>> = OnceLock::new();
    static CACHE: OnceLock<RwLock<HashMap<String, CostPricing>>> = OnceLock::new();
    
    let cache = CACHE.get_or_init(|| RwLock::new(HashMap::new()));
    
    // Fast path: check cache for the model
    if let Ok(guard) = cache.read()
        && let Some(&pricing) = guard.get(model) {
            return pricing;
        }
    
    let rules = RULES.get_or_init(|| {
        vec![
            // ==================== Anthropic Claude Models ====================
            PricingRule::new(r"claude.*opus", 15.00, 75.00),
            PricingRule::new(r"claude.*sonnet", 3.00, 15.00),
            PricingRule::new(r"claude.*haiku", 0.80, 4.00),
            
            // ====================== OpenAI GPT Models ========================
            PricingRule::new(r"gpt-4o-mini", 0.15, 0.60),
            PricingRule::new(r"gpt-4o(?:-202\d-\d\d-\d\d)?", 2.50, 10.00),
            PricingRule::new(r"o1-mini", 1.10, 4.40),
            PricingRule::new(r"o3-mini", 1.10, 4.40),
            PricingRule::new(r"o1(?:-(preview)?.*)?", 15.00, 60.00),
            
            // =================== Google Gemini Models ========================
            PricingRule::new(r"gemini.*pro", 1.25, 10.00),
            PricingRule::new(r"gemini.*flash-lite", 0.10, 0.40),
            PricingRule::new(r"gemini.*flash", 0.30, 2.50),
            
            // ========== Alibaba Qwen Models (USD Approx: CNY / 7.0) =========
            PricingRule::new(r"qwen(?:3)?-max", 0.36, 1.43),
            PricingRule::new(r"qwen(?:3(?:\.[56])?)?-plus", 0.11, 0.29),
            PricingRule::new(r"qwen(?:3(?:\.5)?)?-flash", 0.02, 0.21),
            PricingRule::new(r"qwen(?:2\.5)?-coder", 0.15, 0.71),
            
            // ========== Moonshot Kimi Models (USD Approx: CNY / 7.0) ========
            PricingRule::new(r"kimi.*k2\.5", 0.57, 3.00), // 4 / 21 CNY
            PricingRule::new(r"kimi", 0.57, 2.29),        // 4 / 16 CNY (k2 default)
            
            // ========== Zhipu GLM Models (USD Approx: CNY / 7.0) ============
            PricingRule::new(r"glm-5", 0.57, 2.57),       // 4 / 18 CNY
            PricingRule::new(r"glm-4.*air", 0.11, 0.86),  // 0.8 / 6 CNY
            PricingRule::new(r"glm", 0.43, 2.00),         // 3 / 14 CNY (glm-4 default)

            // ========== MiniMax Models (USD Approx: CNY / 7.0) ==============
            PricingRule::new(r"minimax", 0.30, 1.20),     // 2.1 / 8.4 CNY
        ]
    });

    let result = rules.iter().find_map(|rule| {
        if rule.pattern.is_match(model) {
            Some(rule.costs)
        } else {
            None
        }
    }).unwrap_or(CostPricing {
        input_cost_per_mtoken: 0.0,
        output_cost_per_mtoken: 0.0,
    });

    // Cache the result for future lookups in the same session
    if let Ok(mut guard) = cache.write() {
        guard.insert(model.to_string(), result);
    }

    result
}

pub fn calculate_cost(input_tokens: u32, output_tokens: u32, pricing: &CostPricing) -> f64 {
    let in_cost = (input_tokens as f64 / 1_000_000.0) * pricing.input_cost_per_mtoken;
    let out_cost = (output_tokens as f64 / 1_000_000.0) * pricing.output_cost_per_mtoken;
    in_cost + out_cost
}
