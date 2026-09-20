namespace E2E;

public class Handler
{
    // ARMADILHA C#: o csharp-ls anexa a assinatura ao nome do método no documentSymbol
    // ("DoWork(int x)"). Sem a correção, find_symbol("Handler/DoWork") dava count:0.
    public int DoWork(int x)
    {
        return x + 1;
    }
}

// record top-level para move_symbol: o csharp-ls não implementa "mover para novo arquivo",
// então esperamos o retorno HONESTO move_no_op (não um safe:true fingido).
public record RefundOutcome(int Code);

public static class Usage
{
    public static int Run() => new Handler().DoWork(41);
}
