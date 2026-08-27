use mlua::{Lua, Result, Table};

#[derive(mlua::UserData)]
pub struct Generated {
    #[lua(get, set, name = "display_name")]
    pub name: String,
    #[lua(get)]
    pub count: i64,
}

#[derive(mlua::UserData)]
pub struct Attributed(pub i64);

#[mlua::userdata_impl]
impl Attributed {
    #[lua(getter, name = "value", infallible)]
    fn value(&self) -> i64 {
        self.0
    }

    #[lua(name = "increment", infallible)]
    fn increment(&mut self, amount: i64) -> i64 {
        self.0 += amount;
        self.0
    }

    #[lua(meta, name = "__tostring")]
    fn as_string(&self) -> mlua::Result<String> {
        Ok(self.0.to_string())
    }
}

#[mlua::lua_module]
fn compatibility_fixture(lua: &Lua) -> Result<Table> {
    let module = lua.create_table()?;
    module.set("answer", 42_i64)?;
    Ok(module)
}

#[mlua::lua_module(name = "renamed_fixture")]
fn renamed_entrypoint(lua: &Lua) -> Result<Table> {
    let module = lua.create_table()?;
    module.set("enabled", true)?;
    Ok(module)
}
