# Anchor Desktop

Fatia vertical de comunicacao local do Anchor entre Android e desktop.

## Escopo atual

O desktop inicia um receptor UDP em `0.0.0.0:57421`, aceita trafego em todas as interfaces IPv4, valida `MotionSampleV1` do protocolo v1 e mantem o estado mais recente em memoria com metricas acumuladas.

Esta etapa ainda nao implementa:

- overlay visual;
- autenticacao;
- criptografia.

Esta etapa agora inclui calibracao/zero offline v1 a partir de um dataset `stationary`, um harness offline B3a para avaliar estimadores de roll/pitch e uma selecao offline B3b para produzir uma politica recomendada versionada quando as evidencias locais estao disponiveis. A aplicacao do perfil e dos filtros ao fluxo ao vivo ainda nao existe.

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

Gravacao headless de dataset controlado:

```bash
pnpm motion:record -- --scenario stationary --duration-seconds 15
pnpm motion:analyze -- artifacts/motion-datasets/<arquivo>.ndjson
pnpm motion:calibrate -- artifacts/motion-datasets/<arquivo>-stationary.ndjson
pnpm motion:calibrate -- artifacts/motion-datasets/<arquivo>-stationary.ndjson --output artifacts/motion-calibrations/<perfil>.json --json
pnpm motion:evaluate -- --synthetic --low-pass-tau-ms 50,100,200,400 --complementary-tau-ms 100,250,500,1000
pnpm motion:evaluate -- --dataset artifacts/motion-datasets/<arquivo>.ndjson --profile artifacts/motion-calibrations/<perfil>.json --low-pass-tau-ms 50,100,200,400 --complementary-tau-ms 100,250,500,1000 --json
pnpm motion:select -- --profile artifacts/motion-calibrations/20260902t221420z-stationary-calibration-v1.json --dataset stationary=artifacts/motion-datasets/20260902T221420Z-stationary.ndjson --dataset roll_right=artifacts/motion-datasets/20260902T221919Z-roll_right.ndjson --dataset roll_left=artifacts/motion-datasets/20260902T221937Z-roll_left.ndjson --dataset pitch_front_down=artifacts/motion-datasets/20260902T222109Z-pitch_front_down.ndjson --dataset pitch_front_up=artifacts/motion-datasets/20260902T222126Z-pitch_front_up.ndjson --dataset yaw_clockwise=artifacts/motion-datasets/20260902T222326Z-yaw_clockwise.ndjson --dataset yaw_counterclockwise=artifacts/motion-datasets/20260902T222358Z-yaw_counterclockwise.ndjson --dataset linear_forward=artifacts/motion-datasets/20260902T222523Z-linear_forward.ndjson --dataset linear_backward=artifacts/motion-datasets/20260902T222735Z-linear_backward.ndjson --json
```

O gravador headless reutiliza o mesmo receptor Rust do desktop e deve ser usado com o app Tauri fechado, porque ambos competem pela porta UDP `57421`.

## Calibracao offline v1

A calibracao B2 usa apenas um dataset `stationary` completo e com uma unica sessao para:

- estimar o vetor medio de gravidade no frame do dispositivo;
- calcular uma rotacao ativa `deviceToLeveled` que alinha `normalize(meanGravity)` com `(0, 0, -1)`;
- estimar bias estacionario da aceleracao linear e da velocidade angular no frame do dispositivo;
- gravar um perfil JSON reutilizavel e versionado.

O perfil registra `createdAtUtc` como o instante real da criacao do perfil e preserva o inicio da captura original em `sourceStartedAtUtc`.

Limitacoes deliberadas da v1:

- `yawCalibrated` e sempre `false`;
- nao usa magnetometro;
- nao infere direcao por movimento;
- o resultado e um `leveled mounting frame`, nao um referencial completo do veiculo;
- a operacao nao distingue inclinacao do suporte, do veiculo e do piso/estrada no instante do zero.

Por enquanto, o perfil serve apenas para operacao offline e testes. O receptor, o analyzer B1, o protocolo e o fluxo standalone existente permanecem inalterados.

## Avaliacao offline de filtros B3a

O harness B3a compara, de forma deterministica e offline, tres candidatos minimos para tilt observavel:

- gravidade calibrada sem filtro adicional do Anchor;
- passa-baixa vetorial de primeira ordem parametrizado por `tau`;
- complementar gyro + direcao da gravidade parametrizado por `correctionTau`.

Ele usa `sessionElapsedUs` para calcular `dt` real, reutiliza B1 para carregar datasets e B2 para aplicar o perfil. Na suite sintetica, cada fixture passa por raw -> perfil B2 -> estimador e a saida JSON traz `summary` global por configuracao mais `fixtureResults[]` por fixture. A saida JSON usa `evaluationReportVersion = 1`, nao inclui timestamp atual, nao escolhe vencedor e marca datasets fisicos como `groundTruthAvailable=false`.

Os resultados das capturas fisicas sao proxies comportamentais, nao metricas de acuracia angular. O uso do perfil estacionario nas demais capturas pressupoe o mesmo telefone e montagem preservada.

## Selecao offline de filtros B3b

A B3b adiciona uma camada decisoria separada em `motion_filtering::selection`. Ela reutiliza a B3a, avalia o grid configuravel de parametros, executa os nove datasets fisicos B1 com o perfil B2, aplica gates estruturais, calcula configuracoes dominadas e fronteira de Pareto com tolerancias fisicas declaradas, e emite `selectionReportVersion = 1`.

O contrato `TiltEstimatorPolicyV1` representa a decisao recomendada para uso futuro pelo receptor, mas a B3b nao le esse contrato automaticamente nem altera o fluxo ao vivo. `yawAvailable` permanece sempre `false`.

Metricas de evento no JSON preservam `available`, `failed` ou `unavailable` com unidade e motivo; indisponibilidade aplicavel nao vira `0.0`. Se os artifacts fisicos ignorados pelo Git nao estiverem presentes, `motion:select` retorna erro controlado. As metricas fisicas sao proxies comportamentais, nao acuracia angular. Na execucao real recalculada com as nove capturas locais, a politica recomendada e `gravity_no_additional_anchor_filter` sem parametros; a recomendacao antiga `complementary_tau_ms_400` nao e preservada.

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

## Rotulo de diagnostico

O frontend mostra `Receptor UDP (todas as interfaces IPv4, porta 57421)`, coerente com o bind real em `0.0.0.0:57421`.

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

## Verificacoes relacionadas

```bash
pnpm test:desktop
pnpm test:mobile:standalone-scripts
pnpm verify:mobile:bundle
pnpm build:mobile:standalone
```

- `pnpm test:desktop` cobre os testes TypeScript do frontend desktop, incluindo o rotulo do receptor.
- `pnpm test:mobile:standalone-scripts` cobre a selecao de JDK e build-tools usada pelos scripts standalone.
- `pnpm verify:mobile:bundle` valida a resolucao Metro real do monorepo.
- `pnpm build:mobile:standalone` produz o APK interno Android sem depender do Metro.

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
├── calibration/
│   └── mod.rs
├── dataset/
│   └── mod.rs
├── lib.rs
├── main.rs
├── motion_filtering/
│   ├── estimator.rs
│   ├── metrics.rs
│   ├── mod.rs
│   ├── report.rs
│   ├── selection.rs
│   └── synthetic.rs
├── protocol.rs
└── receiver/
    ├── mod.rs
    └── udp.rs
```
