include!("v10.rs");

pub struct Owned(pub i64);

impl mlua::UserData for Owned {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_once("consume", |_, this, ()| Ok(this.0));
        methods.add_async_method_once("consume_async", |_, this, ()| async move { Ok(this.0) });
    }
}
