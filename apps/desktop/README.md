# Anchor Desktop

Fatia vertical de comunicacao local do Anchor entre Android e desktop.

## Escopo atual

O desktop inicia um receptor UDP em `0.0.0.0:57421`, aceita trafego em todas as interfaces IPv4, valida `MotionSampleV1` do protocolo v1 e mantem o estado mais recente em memoria com metricas acumuladas.

Esta etapa ainda nao implementa:

- overlay visual;
- autenticacao;
- criptografia.

## Como executar

Terminal 1:

```bash
pnpm dev:desktop
```

Terminal 2:

```bash
pnpm dev:simulator
```

Execucao por tempo limitado:

```bash
pnpm dev:simulator -- --duration 5 --pattern sine
```

## Endereco e porta padrao

- Bind: `0.0.0.0`
- Porta: `57421`
- Taxa esperada do stream: `60 Hz`
- Limite tecnico de processamento: `240 datagramas por segundo`

O simulador continua funcionando contra `127.0.0.1:57421`, porque o bind em `0.0.0.0` tambem recebe datagramas enviados ao loopback local.

Nao ha descoberta automatica do IP LAN nesta rodada. O firewall tambem nao e alterado automaticamente.

## Estado do receptor

O estado em memoria mantem:

- ultima amostra valida;
- emissor ativo;
- `sessionId` ativo;
- ultimo `sequence` aceito;
- instante local da ultima recepcao valida;
- metricas acumuladas.

## Status do stream

- `active`: recebeu amostra valida nos ultimos `250 ms`;
- `stale`: nao recebeu amostra valida por mais de `250 ms`;
- `disconnected`: nao recebeu amostra valida por mais de `1 s`.

A ultima amostra nao e apagada imediatamente quando o stream fica `stale` ou `disconnected`.

## Regras de sessao e ordenacao

- a primeira amostra valida estabelece a sessao ativa;
- a mesma sessao so aceita `sequence` estritamente crescente;
- pacotes duplicados ou fora de ordem sao ignorados e contabilizados;
- outra sessao so pode assumir depois de `1 s` sem amostra valida;
- rollover de `u32` nao e tratado neste MVP e permanece uma decisao deliberada documentada.

## Metricas

- `received_datagrams`: datagramas recebidos pelo socket;
- `accepted_samples`: amostras validas aceitas no estado;
- `oversized_datagrams`: datagramas acima de `1024` bytes;
- `invalid_packets`: JSON invalido, estrutura invalida ou validacao semantica rejeitada;
- `duplicate_or_out_of_order_packets`: sequencias repetidas ou antigas;
- `foreign_session_packets`: pacotes ignorados por pertencerem a outra sessao enquanto a atual segue ativa;
- `rate_limited_datagrams`: pacotes descartados pelo limitador tecnico antes do parse.

Os logs fazem resumo dessas metricas no maximo uma vez por segundo. Nao ha log por pacote.

## Seguranca desta fase

- UDP nesta versao nao possui ACK;
- UDP nesta versao nao possui autenticacao;
- UDP nesta versao nao possui criptografia;
- use o firewall do sistema para restringir a porta `57421` a LAN quando necessario;
- nao exponha a porta diretamente a internet.

## Validacao manual do fluxo Android -> desktop

1. Descubra manualmente o IPv4 LAN do computador.
2. Inicie o desktop com `pnpm dev:desktop`.
3. Confirme nos logs: `motion receiver listening on 0.0.0.0:57421 across all IPv4 interfaces`.
4. Instale e abra o APK Android.
5. Informe no celular o IPv4 do computador e a porta `57421`.
6. Inicie o streaming.
7. Confirme no desktop:
   - sender correspondente ao celular;
   - mesma `sessionId` do mobile;
   - `sequence` crescente;
   - taxa proxima de `60 Hz`;
   - `received` e `accepted` crescendo.
8. Pare no celular.
9. Confirme a transicao para `stale` e depois `disconnected`.
10. Inicie novamente no celular e confirme nova `sessionId`.

## Estrutura Rust

```text
apps/desktop/src-tauri/src/
├── lib.rs
├── main.rs
├── protocol.rs
└── receiver/
    ├── mod.rs
    └── udp.rs
```
