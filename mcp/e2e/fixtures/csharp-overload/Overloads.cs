namespace E2E;

public enum Color { Red, Green }
public enum Size { S, M }

public static class Ext
{
    // 2 overloads homônimos com tipos DIFERENTES (overload legítimo).
    public static string ToText(this Color v) => v.ToString();   // linha 9
    public static string ToText(this Size v) => v.ToString();    // linha 10
}

public static class Usage
{
    public static string A() => Color.Red.ToText();  // usa o overload de Color
    public static string B() => Size.M.ToText();      // usa o overload de Size
}
