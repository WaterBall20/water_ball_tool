use super::data::{FieldCard, FieldCardEffect, SkillCard};
use std::sync::Arc;
struct GameSimulator {}

#[derive(Clone)]
struct AllCardList {
    skill_card: Arc<Vec<SkillCard>>,
    field_card: Arc<Vec<FieldCard>>,
}

struct CardList {
    all_card_list: AllCardList,
    skill_card_index: Vec<usize>,
    field_card_index: Vec<usize>,
}
impl CardList {
    pub fn get_skill(&self, index: usize) -> Option<&SkillCard> {
        self.all_card_list
            .skill_card
            .get(*self.skill_card_index.get(index)?)
    }

    pub fn get_field(&self, index: usize) -> Option<&FieldCard> {
        self.all_card_list
            .field_card
            .get(*self.field_card_index.get(index)?)
    }

    pub fn get_skill_list(&self) -> Vec<&SkillCard> {
        let mut r = Vec::with_capacity(self.skill_card_index.len());
        for &index in &self.skill_card_index {
            r.push(&self.all_card_list.skill_card[index]);
        }
        r
    }

    pub fn get_field_list(&self) -> Vec<&FieldCard> {
        let mut r = Vec::with_capacity(self.field_card_index.len());
        for &index in &self.field_card_index {
            r.push(&self.all_card_list.field_card[index]);
        }
        r
    }
}

//游戏运行时
struct GameRuntime {
    data: Arc<Data>,
}
impl GameRuntime {
    pub fn get_init_state(&self) -> GameState {
        todo!()
    }
}

struct Data {
    dance: f32,
    visual: f32,
    vocal: f32,
    type2_list: Vec<Type2>,
    turn_count: usize,
    card_list: CardList,
}

enum Type2 {
    Dance,
    Visual,
    Vocal,
}

//
struct GameState {
    data: Arc<Data>,
    turn_index: usize,
    all_card_list: AllCardList,
    field_effects: Vec<FieldCardEffect>,
    list_a: Vec<usize>, // 当前持有
    list_b: Vec<usize>, //
    list_c: Vec<usize>, //
    list_d: Vec<usize>, // 排除
}

impl GameState {
    fn new(
        data: Arc<Data>,
        turn_index: usize,
        all_card_list: AllCardList,
        field_effects: Vec<FieldCardEffect>,
        list_b: Vec<usize>,
        list_c: Vec<usize>,
        list_d: Vec<usize>,
    ) -> Self {
        let mut list_a = Vec::with_capacity(5);
        let mut list_b = list_b;
        let mut list_c = list_c;
        for _ in 0..(list_b.len() + list_c.len()).min(3) {
            if list_b.is_empty() {
                std::mem::swap(&mut list_b, &mut list_c);
            }
            list_a.push(list_b.remove(rand::random_range(0..list_b.len())));
        }
        Self {
            data,
            turn_index,
            all_card_list,
            field_effects,
            list_a,
            list_b,
            list_c,
            list_d,
        }
    }

    pub fn next(&self, list_a_index: usize) -> Option<GameState> {
        let &this_list_a_value = self.list_a.get(list_a_index)?;
        let this_card = self.all_card_list.skill_card.get(this_list_a_value)?;
        let list_b = self.list_b.clone();
        let mut list_c = self.list_c.clone();
        let mut list_d = self.list_d.clone();
        let mut field_effects = self.field_effects.clone();
        //TODO:实现效果处理
        for (this_index, &a_item) in self.list_a.iter().enumerate() {
            if this_index != list_a_index {
                list_c.push(a_item);
            }
        }
        Some(Self::new(
            self.data.clone(),
            self.turn_index + 1,
            self.all_card_list.clone(),
            field_effects,
            list_b,
            list_c,
            list_d,
        ))
    }

    pub fn get_list_inner<'a>(
        all_card_list: &'a AllCardList,
        list: &'a Vec<usize>,
    ) -> Vec<&'a SkillCard> {
        let mut r = Vec::with_capacity(list.len());
        for &i in list {
            r.push(&all_card_list.skill_card[i]);
        }
        r
    }

    pub fn get_list_a(&self) -> Vec<&SkillCard> {
        Self::get_list_inner(&self.all_card_list, &self.list_a)
    }

    pub fn get_list_b(&self) -> Vec<&SkillCard> {
        Self::get_list_inner(&self.all_card_list, &self.list_b)
    }

    pub fn get_list_c(&self) -> Vec<&SkillCard> {
        Self::get_list_inner(&self.all_card_list, &self.list_c)
    }

    pub fn get_list_d(&self) -> Vec<&SkillCard> {
        Self::get_list_inner(&self.all_card_list, &self.list_d)
    }
}
