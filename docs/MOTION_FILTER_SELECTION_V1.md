# Motion Filter Selection V1

Selecao offline deterministica da fatia B3b para escolher, quando a evidencia permitir, uma politica recomendada de estimador de roll/pitch observavel para futura producao.

## Objetivo e fronteira

A B3b consome a bancada B3a sem alterar suas formulas. Ela compara configuracoes sobre a suite sintetica com ground truth e sobre as nove capturas fisicas B1 usando apenas proxies comportamentais.

Fora desta fatia:

- aplicar filtro, calibracao ou politica ao receptor ao vivo;
- estimar yaw;
- usar magnetometro, Kalman ou correcao adaptativa por aceleracao linear;
- implementar UI, overlay, politica de perda, jitter, stale ou disconnect.

## Entradas

Suite sintetica:

- `synthetic-suite-v1` fornecida pela B3a;
- candidatos: gravidade sem filtro adicional, passa-baixa vetorial e complementar gravity + gyro.

Grid padrao da B3b:

- low-pass `tauMs`: `25,50,75,100,150,200,300,400,600,800`;
- complementar `correctionTauMs`: `50,75,100,150,250,400,600,1000,1500,2000`.

Capturas fisicas B1 esperadas:

- `stationary=artifacts/motion-datasets/20260902T221420Z-stationary.ndjson`;
- `roll_right=artifacts/motion-datasets/20260902T221919Z-roll_right.ndjson`;
- `roll_left=artifacts/motion-datasets/20260902T221937Z-roll_left.ndjson`;
- `pitch_front_down=artifacts/motion-datasets/20260902T222109Z-pitch_front_down.ndjson`;
- `pitch_front_up=artifacts/motion-datasets/20260902T222126Z-pitch_front_up.ndjson`;
- `yaw_clockwise=artifacts/motion-datasets/20260902T222326Z-yaw_clockwise.ndjson`;
- `yaw_counterclockwise=artifacts/motion-datasets/20260902T222358Z-yaw_counterclockwise.ndjson`;
- `linear_forward=artifacts/motion-datasets/20260902T222523Z-linear_forward.ndjson`;
- `linear_backward=artifacts/motion-datasets/20260902T222735Z-linear_backward.ndjson`.

Perfil B2 esperado:

- `artifacts/motion-calibrations/20260902t221420z-stationary-calibration-v1.json`.

Premissas para reutilizar o perfil nas nove capturas:

- mesmo telefone;
- mesma convencao de montagem;
- montagem fisica materialmente preservada.

## Metodologia

A selecao executa o mesmo pipeline para todos os candidatos:

```text
raw sample -> calibracao B2 -> estimador B3a -> metricas -> gates -> Pareto -> desempate documentado
```

Gates obrigatorios:

- nenhuma falha de avaliacao, panic ou numero nao finito;
- fixtures obrigatorias presentes, incluindo `irregular_dt` e `mounting_bias_b2`;
- metricas aplicaveis aos eventos declarados representadas com estado explicito;
- politica final validavel com `yawAvailable=false`;
- entrada de avaliacao com `yawCalibrated=false`;
- configuracao e parametros validos;
- pipeline raw -> perfil B2 -> estimador B3a executado.

Os limites numericos de desempenho nao sao usados como prova de contrato. As verificacoes analiticas da B3a cobrem uso correto de `dt`, normalizacao e convencoes matematicas; a B3b apenas exige que as fixtures correspondentes sejam executadas e que as metricas sejam finitas quando disponiveis.

Metricas temporais e de evento usam contrato explicito:

- `available`: valor finito e unidade;
- `failed`: fixture aplicavel declarou o evento, mas a metrica obrigatoria nao ficou disponivel; inclui `reason`;
- `unavailable`: metrica nao aplicavel aquela fixture ou dimensao.

Ausencia de metrica aplicavel nunca vira `0.0`. Uma falha nao pode ser melhor que valor finito em Pareto ou desempate.

Tolerancias fisicas usadas para equivalencia:

- sine lag: `0.001 s`;
- settling/recovery: `0.01 s`;
- erros angulares sinteticos e proxies angulares fisicos: `0.01 deg`;
- taxa angular fisica proxy: `0.01 deg/s`.

Metricas sinteticas com ground truth permanecem separadas de proxies fisicos. O algoritmo nao calcula media entre unidades diferentes e nao usa soma ponderada opaca.

## Prioridades de decisao

Antes da recomendacao, a B3b aplica estas prioridades:

1. estabilidade numerica e respeito ao contrato;
2. rastreamento dinamico com baixo atraso;
3. resistencia a contaminacao transitoria da gravidade;
4. correcao de drift giroscopico;
5. estabilidade em repouso;
6. simplicidade e custo previsivel.

## Evidencias

Matriz resumida produzida pelo JSON B3b:

| Categoria | Uso na decisao | Observacao |
|---|---|---|
| RMSE, p95 e max angular sintetico | Pareto e rationale | Ground truth sintetico |
| Erro de roll/pitch | Pareto | Ground truth sintetico |
| Overshoot, settling e lag | Pareto e desempate | Mantidos por unidade |
| Drift com bias de gyro | Pareto e desempate | Evidencia sintetica |
| Pulso de gravidade RMSE/max/recovery | Pareto e desempate de prioridade composta | Evidencia sintetica; conflitos nao sao resolvidos por pesos |
| `irregular_dt` | Gate estrutural e Pareto | Fixture executada; desempenho nao prova contrato |
| `mounting_bias_b2` | Gate estrutural e Pareto | Fixture executada; desempenho nao prova convencao |
| Captura fisica `stationary` | Desempate tardio como proxy | Nunca precisao ou acuracia angular |
| Demais capturas fisicas | Observacional | Regressao/sensibilidade aparente; nao entram silenciosamente em score |

## Fronteira de Pareto

Configuracoes dominadas sao aquelas em que outra configuracao elegivel e melhor ou igual em todas as dimensoes sinteticas comparaveis e estritamente melhor em pelo menos uma.

A dominancia usa apenas dimensoes semanticamente equivalentes e as tolerancias fisicas declaradas. Valores `available` sao preferidos a `failed` na mesma dimensao; duas falhas podem empatar, mas nenhuma falha vence valor finito. Configuracoes inelegiveis nao entram em `paretoFront`.

O campo JSON `paretoFront` contem os IDs das configuracoes elegiveis nao dominadas. A fronteira e deterministica para o mesmo conjunto de entradas.

Na execucao real com as nove capturas B1 e o perfil B2 selecionado, a fronteira foi:

- `gravity_no_additional_anchor_filter`;
- `low_pass_tau_ms_25`;
- `low_pass_tau_ms_50`;
- `low_pass_tau_ms_75`;
- `low_pass_tau_ms_100`;
- `low_pass_tau_ms_150`;
- `low_pass_tau_ms_200`;
- `low_pass_tau_ms_300`;
- `low_pass_tau_ms_400`;
- `low_pass_tau_ms_600`;
- `low_pass_tau_ms_800`;
- `complementary_tau_ms_50`;
- `complementary_tau_ms_75`;
- `complementary_tau_ms_100`;
- `complementary_tau_ms_150`;
- `complementary_tau_ms_250`;
- `complementary_tau_ms_400`;
- `complementary_tau_ms_600`;
- `complementary_tau_ms_1000`;
- `complementary_tau_ms_1500`;
- `complementary_tau_ms_2000`.

## Configuracao selecionada

O contrato permite dois estados:

- `selected`: ha uma recomendacao unica defensavel;
- `inconclusive`: a evidencia nao permite escolher sem captura adicional.

Resultado da execucao real B3b:

- `status`: `selected`;
- configuracao: `gravity_no_additional_anchor_filter`;
- contrato: `TiltEstimatorPolicyV1`;
- `candidate`: `gravity_no_additional_anchor_filter`;
- `parameters`: `{}`;
- `yawAvailable`: `false`;
- `source`: `b3b_offline_selection`.

Rationale registrado no JSON:

- elegivel por todos os gates B3b;
- presente na fronteira de Pareto sob metricas sinteticas comparaveis;
- prioridades aplicaram tolerancias fisicas antes de avancar: lag, contaminacao da gravidade, drift, proxy estacionario e simplicidade.

A decisao antiga `complementary_tau_ms_400` foi recalculada e nao e preservada por compatibilidade. Lags numericos da ordem de `10^-15 s` sao tratados como equivalentes fisicamente.

## Alternativas e trade-offs

O relatorio JSON inclui:

- `alternativesNearby` para configuracoes proximas no desempate;
- `losesOn` para metricas em que a escolha perde;
- `knownRisks` e `reviewConditions` para revisao futura.

Alternativas proximas na execucao real:

- `low_pass_tau_ms_25`;
- `low_pass_tau_ms_50`;
- `low_pass_tau_ms_75`.

A configuracao escolhida perde, segundo o proprio relatorio, em:

- RMSE angular sintetico;
- dispersao fisica estacionaria proxy.

## Riscos e limitacoes

- `TYPE_GRAVITY` do Android pode conter filtragem ou fusao do fabricante.
- Capturas fisicas B1 nao fornecem ground truth angular.
- Cenarios `linear_forward` e `linear_backward` indicam sensibilidade comportamental a aceleracao transitoria, mas nao provam erro angular real.
- Yaw permanece indisponivel.
- A politica B3b nao e lida automaticamente pelo receptor.

## Comando reproduzivel

```bash
pnpm motion:select -- \
  --profile artifacts/motion-calibrations/20260902t221420z-stationary-calibration-v1.json \
  --dataset stationary=artifacts/motion-datasets/20260902T221420Z-stationary.ndjson \
  --dataset roll_right=artifacts/motion-datasets/20260902T221919Z-roll_right.ndjson \
  --dataset roll_left=artifacts/motion-datasets/20260902T221937Z-roll_left.ndjson \
  --dataset pitch_front_down=artifacts/motion-datasets/20260902T222109Z-pitch_front_down.ndjson \
  --dataset pitch_front_up=artifacts/motion-datasets/20260902T222126Z-pitch_front_up.ndjson \
  --dataset yaw_clockwise=artifacts/motion-datasets/20260902T222326Z-yaw_clockwise.ndjson \
  --dataset yaw_counterclockwise=artifacts/motion-datasets/20260902T222358Z-yaw_counterclockwise.ndjson \
  --dataset linear_forward=artifacts/motion-datasets/20260902T222523Z-linear_forward.ndjson \
  --dataset linear_backward=artifacts/motion-datasets/20260902T222735Z-linear_backward.ndjson \
  --low-pass-tau-ms 25,50,75,100,150,200,300,400,600,800 \
  --complementary-tau-ms 50,75,100,150,250,400,600,1000,1500,2000 \
  --json
```

Duas execucoes identicas devem produzir JSON byte a byte identico. Caminhos no relatorio sao nomes logicos ou basenames, nunca caminhos absolutos.

## Relacao B3a -> B3b -> proxima fatia

B3a fornece estimadores, fixtures, metricas e replay. B3b adiciona decisao offline, Pareto, gates e contrato versionado `TiltEstimatorPolicyV1`. A proxima fatia pode consumir essa politica, mas ainda precisa definir comportamento para perda, gaps, stale, jitter e integracao ao receptor ao vivo.
