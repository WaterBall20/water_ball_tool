//数据结构体和枚举
use std::sync::Arc;

/*
* 关于数字类型的说明
* 为简化计算转换，只要参与最终“数值”的计算，均为f32
*   其余如次数，可使用usize，用于循环
*/

//卡牌类型
enum CardType {
    Skill(Box<CardType>), //技能卡
    ActiveSkill,          //
    MentalSkill,
}

//指针类型
enum Cuideline {
    Confident(f32),      //强气
    FullPower,           //全力
    FullPowerPoint(f32), //全力值
    Preservation(f32),   //温存
}

//属性类型
enum Attribute {
    //=== 角色属性 ===
    Vocal, //歌唱
    Visual,
    Dance, //舞蹈
    //参数
    Parameter { value: f32, count: usize },
    Stamina(f32),                                      //体力
    StaminaConsumptionCut { value: f32 },              //消费体力削减
    StaminaConsumptionReduction { turn_count: usize }, //消费体力减少
    AdditionalStaminaConsumption { value: f32 },       //消费体力追加
    IncreasedStaminaConsumption { turn_count: usize }, //消费体力追加
    Energy(f32),                                       //元气
    Concentration(f32),                                //集中
    Tuning(f32),                                       //好调
    Imperial(f32),                                     //好印象
    Motivation(f32),                                   //干劲
    Guideline(Option<Cuideline>),                      //指针
    NegateDecreaseState(usize),                        //低下状态无效
    FixedGuideline(usize),                             //指针固定
    PoorCondition { turn_count: usize },               //不调
    Anxiety { turn_count: usize },                     //不安 TODO:需要检查效果
    Timid { turn_count: usize },                       //弱气 TODO:需要验证效果
}
//效果类型
enum Effect {
    //属性变更
    Attribute(Attribute),
    //含条件的效果
    Conditions {
        r#if: EffectConditions,
        effect_type: Box<EffectConditions>,
    },
}
//消费类型
enum ConsumptionType {
    //体力
    Stamina {
        value: f32,
        is_consumption: bool, //是否体力消费
    },
    Concentration(f32),  //集中
    Tuning(f32),         //好调,
    Impression(f32),     //好印象
    Motivation(f32),     //干劲
    FullPowerValue(f32), //全力值
}

//效果条件类型
enum EffectConditions {
    //=== 比较类型 ===
    //偶像技能卡卡牌
    IdolSkillCard {
        count: usize,                //次数
        card: Arc<SkillCardRunning>, //技能卡
    },
}

//触发类型
enum TriggerTiming {
    TurnStart, //回合开始
    TurnEnd, //回合结束
}

//运行时效果
enum CardRunningEffect {}

//技能卡
struct SkillCard {
    name: String,
    consumption_type: ConsumptionType, //消费类型
    effects: Vec<Effect>,              //卡牌效果
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
