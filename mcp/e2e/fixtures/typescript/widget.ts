// Duas classes com um método HOMÔNIMO (render): find_symbol("Widget/render") deve casar SÓ o de
// Widget (precisão do name_path composto), não o de Gadget.
export class Widget {
  render(): string {
    return "widget";
  }
}

export class Gadget {
  render(): string {
    return "gadget";
  }
}
