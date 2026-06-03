use std::sync::Arc;

//流派类型
enum CardType {
    Emotion, //感性
    Logic, //理性
    Esper, //非凡
    Neutral, //通用
}
//指针类型
enum PointerStateType {
    None, //没有
    Some, //存在
    Strong(u8), //强气
    FullPower, //全力
    WarmUp(u8), //温存
}
//效果类型
enum EffectType {
    //数值
    NumberValue {
        value: u8,
        count: u8,
    },
    Stamina(i8), //体力
    //元气
    Vigour {
        value: i16,
        sum_all: bool,
    },
    Concentration(i16), //集中
    Tuning(i16), //好调
    Imperial(i16), //好印象
    Motivation(u16), //干劲
    FullPowerValue(i8), //全力值
    Pointer(PointerStateType), //指针
    Turn(i8), //回合
}
//消费类型
enum ConsumptionType {
    //体力
    Stamina {
        value: u8,
        no_vigour: bool, //不使用元气，体力消费
    },
    Concentration(u8), //集中
    Tuning(u8), //好调,
    Impression(u8), //好印象
    Motivation(u8), //干劲
    FullPowerValue(u8), //全力值
}

//效果条件类型
enum EffectIfType {}
//卡牌效果
struct CardEffect {
    card_effect_type: EffectType,
    effect_if_type: EffectIfType,
}
//运行时效果
enum CardRunningEffect {}

//技能卡
struct SkillCard {
    name: String,
    consumption_type: ConsumptionType, //消费类型
    card_effects: Vec<CardEffect>, //卡牌效果
    use_count: Option<u8>, //使用次数
}

//技能卡运行时
struct SkillCardRunning {
    id: String,
    skill_card: Arc<SkillCard>,
    card_running_effects: Vec<CardRunningEffect>,
}

//场地效果卡
struct FieldEffectCard {
    name: String,
}

//场地效果卡运行时
struct FieldEffectCardRunning {
    id: String,
    field_effect_card: Arc<FieldEffectCard>,
}
