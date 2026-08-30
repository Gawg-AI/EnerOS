//! 外部 crate 无法发放认证权印：CertifierSeal 无公开构造器——
//! 生成器（solver/规则/模型）不能给自己签证书（[02] §7 表禁止行为）。
use eneros_decision_contract::CertifierSeal;

fn main() {
    let seal = CertifierSeal { _priv: () };
    let _ = seal;
}
