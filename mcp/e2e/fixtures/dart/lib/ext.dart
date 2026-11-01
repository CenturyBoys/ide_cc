/// Fixture p/ N5 (issue #4): extension/mixin devem ter `kind` consistente entre
/// document_symbols e workspace_symbols.
extension IntDoubler on int {
  int get doubled => this * 2;
}

mixin Walks {
  String walk() => 'step';
}
