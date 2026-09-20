// Classe ANOTADA (annotation built-in, sem import de pacote): guarda contra a armadilha de posição
// (em Dart resolve no identificador, não na anotação — o e2e confirma que continua assim).
@Deprecated('use FeatureFlag')
class Flag {
  final String name;
  const Flag(this.name);

  String label() => name;
}

class FeatureFlag {
  final String key;
  const FeatureFlag(this.key);
}

Flag makeFlag(String n) => Flag(n);

String useFlag() => makeFlag('x').label();
