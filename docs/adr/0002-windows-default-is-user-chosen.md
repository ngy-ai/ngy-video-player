# Windows 上引导用户设默认，不写 UserChoice

Windows 8 起 `Explorer\FileExts\<ext>\UserChoice` 带哈希校验。程序自己写入会被系统判定为篡改：关联被静默撤销，并弹一条"默认应用已重置"。我们只做合法的那一半——注册 ProgID 与 `Capabilities`，让本程序出现在"默认应用"列表里，然后把用户送到系统设置页，由用户亲手选定。

这条必须写下来，因为"帮用户少点两下"的直觉会反复诱惑后来者直接去写 `UserChoice`。那样换来的不是默认地位，而是每次开机都弹一次重置提示。

## Considered Options

- `IApplicationAssociationRegistrationUI::LaunchAdvancedAssociationUI`：Win11 会重定向到设置页，行为随版本漂移，只作为打开设置页的备选。
- 伪造 `UserChoice` 哈希：会被系统重置，放弃。
