# Motion Latency Measurement V1 — B5b

## Escopo

B5b é um modo diagnóstico explicitamente opt-in que mede aproximações entre o evento de sensor Android, a preparação nativa do envio, o ingresso UDP no Rust e a conclusão do processamento B4b. Resultados carregam intervalo e incerteza de sincronização. Não existe medição sensor → tela, sensor-to-photon, resposta visual completa ou latência clínica, nem veredito de desempenho.

O stream normal continua enviando `MotionSampleV1` v1 sem campos novos. B5b usa socket bidirecional e protocolo auxiliar v1 exclusivos (`latency_hello_v1`, `clock_sync_request_v1`, `clock_sync_response_v1`, `latency_sample_v1`), limite de 4096 bytes, JSON estrito e `measurementId` opaco. B5a permanece separada.

## Timestamps

- `androidSensorEventUs`: `SensorEvent.timestamp / 1000` do giroscópio que dispara o frame, no domínio monotônico de `elapsedRealtimeNanos`; é preservado antes da agregação/bridge.
- `androidSendPreparationUs`: `elapsedRealtimeNanos / 1000` no worker nativo, imediatamente antes de construir e enviar `latency_sample_v1`; não é o instante físico de transmissão. O intervalo inclui agregação, bridge React Native, validação/serialização JS e espera do sender.
- `t1DesktopUs`: imediatamente antes de `send_to` do probe.
- `t2AndroidUs`: imediatamente após `receive` do probe no socket B5b.
- `t3AndroidUs`: imediatamente antes da construção/envio da resposta.
- `t4DesktopUs`: imediatamente após `recv_from` da resposta.
- `desktopIngressUs`: imediatamente após `recv_from` do datagrama de amostra.
- `desktopAcceptedUs`: após validação e aceitação de sessão/sequência.
- `desktopProcessedUs`: após retorno de `LiveMotionPipeline::process`, sem polling Tauri.

Todos são microssegundos monotônicos nos respectivos dispositivos. As épocas Android e desktop são independentes e nunca são diretamente subtraídas.

## Sincronização e métricas

Para cada troca válida:

```text
networkRttUs = (t4 - t1) - (t3 - t2)
offsetMidpointUs = ((t2 - t1) + (t3 - t4)) / 2
offsetLowerUs = t3 - t4
offsetUpperUs = t2 - t1
offsetUncertaintyUs = (offsetUpperUs - offsetLowerUs) / 2
symmetricOneWayEstimateUs = networkRttUs / 2
```

`androidClock = desktopClock + offset`; portanto `estimatedDesktopTime = androidTime - offsetMidpoint`. `RTT / 2` é somente estimativa sob hipótese de caminhos simétricos. Offset e incerteza são interpolados linearmente apenas entre âncoras que envolvem a amostra; não há extrapolação ou clamp.

As cinco métricas são `androidSensorToSendUs`, `androidSendToDesktopIngressEstimateUs`, `desktopIngressToProcessedTiltUs`, `sensorToDesktopIngressEstimateUs` e `sensorToProcessedTiltEstimateUs`. Métricas convertidas incluem estimativa, limites, incerteza e IDs das âncoras. Indisponibilidade traz motivo, nunca zero artificial.

## Operação

```powershell
pnpm motion:latency -- --duration-seconds 60 --profile <perfil.json> --policy <politica-v2.json>
```

Opções: `--startup-timeout-seconds`, `--final-sync-timeout-seconds`, `--bind`, `--dataset-output` e `--report-output`. Defaults: duração 60 s, timeout inicial 60 s, deadline final 5 s, bind `0.0.0.0:57421` e diretórios ignorados `artifacts/motion-latency-datasets/` e `artifacts/motion-latency-measurements/`. Saídas existentes não são sobrescritas.

O Android deve marcar “Ativar transporte bidirecional exclusivo de latência”. O desktop faz o bind antes de criar qualquer dataset e, em seguida, valida o perfil B2 e a política B3b V2 antes de iniciar a captura. A janela começa na primeira amostra aceita depois de uma âncora válida. Probes ocorrem antes/durante/depois; uma âncora final deve cobrir a janela.

Ctrl+C antes da primeira amostra remove dataset vazio e não cria relatório. Durante a janela preserva dados, produz relatório analisável e reprova duração/conclusão. Códigos: `0` gates aprovados; `2` relatório válido com gates reprovados; `1` erro operacional/dados insuficientes.

## Dataset, relatório e gates

O NDJSON `latencyDatasetVersion=1` passa por um recorder bounded único, cujo worker é o único proprietário do arquivo. Ele registra metadata, hello, requests/responses brutos, rejeições e fatos separados `sampleAccepted`/`sampleProcessed`; o shutdown drena os produtores e escreve o summary por último. A metadata e o relatório preservam o contrato B4b completo usado na execução: caminhos, `CalibrationProfileFingerprintV1` estruturado e toda a política B3b V2, incluindo candidato, parâmetros, versão, origem, `yawAvailable` e fingerprint vinculado.

A análise offline usa JSON estrito, exige metadata/hello/summary nas posições válidas, confere versões, measurement IDs, endpoints, nonces, timestamps, duplicações, correspondência accepted/processed e o vínculo entre os fingerprints do perfil e da política. Ela pareia probes, recalcula âncoras, duração e contadores a partir dos fatos. Drops do recorder, que não podem aparecer como records gravados, são preservados do summary; os demais contadores são recompostos e comparados à declaração operacional. O JSON `latencyMeasurementReportVersion=1` é derivado dessa análise validada; distribuições e `observedOffsetChangeUs` usam disponibilidade explícita, nunca zero artificial.

O sender Android B5b usa latest-wins com capacidade pendente 1. Cada substituição é descarte de backpressure contabilizado; `offeredSamples`, `sentSamples`, `droppedBackpressure`, `rejectedSamples` e `sendErrors` são reiniciados por geração e expostos na UI. O stream v1 normal permanece separado.

Os gates comuns de integridade de B5a e B5b são produzidos pelo mesmo avaliador compartilhado: duração mínima; sessão única e sem transições; taxa de origem `60 Hz ±5%`; coerência de taxa recebida/perda `±5%`; perda por sequência ≤ `0,1%`; zero oversized, inválidos, duplicados/fora de ordem, sessão estrangeira, rate limited e drops do recorder; correspondência entre amostras aceitas no receiver e registradas; e conclusão limpa. B5b acrescenta hello válido/único; identidade de endpoint/measurement consistente; respostas pareadas; timestamps monotônicos; pelo menos duas âncoras; cobertura integral sem extrapolação; correspondência amostra/processamento; contrato B4b; zero descartes runtime/publicação de resultados; consistência do summary; e distribuições essenciais finitas. RTT, percentis, máximos, estimativas e incerteza não têm threshold.

## Estado

A revisão automatizada cobre agora a matriz de lifecycle e análise offline: Ctrl+C antes/durante, timeout inicial e final determinístico, filas runtime/result/recorder cheias ou fechadas, falhas dos workers, proteção contra overwrite, bind ocupado sem artefato residual, JSON truncado/campo desconhecido, divergências de versão/measurement/endpoint/fingerprint, duplicações e determinismo byte a byte do relatório completo.

B5b foi validada fisicamente em 22 de outubro de 2026. A implementação e os 26 gates da execução física foram aprovados; com isso, B5b e a Fase B estão concluídas. A Fase C ainda não começou.

## Validação física aprovada — 22 de outubro de 2026

O ensaio aprovado foi executado em Windows com o APK Android standalone, Wi-Fi/LAN local e receiver headless Rust. O celular usado foi o Android `2602DPT53G` (`chagall`), conectado ao desktop `192.168.1.209:57421`. A configuração B4b preservada no relatório usa o perfil `artifacts/motion-calibrations/20261001T092624Z-stationary-calibration-v1.json`, a política `artifacts/motion-filtering/20261001T094258Z-tilt-estimator-policy-v2.json`, o candidato `gravity_no_additional_anchor_filter` e o fingerprint SHA-256 `43e9378ae7290c57bd4dbc6b8a2259a3126a4f0159077e268fa0ec2fa07d5797`.

O relatório aprovado é `artifacts/motion-latency-measurements/20261022T124629Z-b5b-v1.json`, derivado do dataset `artifacts/motion-latency-datasets/20261022T124629Z-b5b-v1.ndjson`; ambos permanecem locais e ignorados pelo Git. O `measurementId` é `b5b-1792673207200-350c22b5edeac4`. A janela solicitada foi de `60.000.000 us`, a duração observada foi de `60.003.054 us` e foram aceitas e processadas `3.601` amostras. Os 26 gates passaram, sem gates reprovados.

Os contadores reconstruídos registraram `3.736` datagramas, `122/122` probes enviados/respondidos, zero probes perdidos ou rejeitados, zero pacotes inválidos, oversized, estrangeiros, duplicados/fora de ordem, de sessão estrangeira ou limitados por taxa, e zero drops do runtime, publicação de resultados ou recorder. Doze amostras chegaram após a janela nominal e foram preservadas explicitamente como `sample_after_window`; elas entram na contabilidade de datagramas, mas não na janela de `3.601` amostras.

As principais distribuições exploratórias foram:

| Distribuição | p50 | p95 | p99 |
|---|---:|---:|---:|
| RTT de rede | `4.423,5 us` | `21.490,65 us` | `46.365,21 us` |
| Estimativa de rede em um sentido, simétrica | `2.211,75 us` | `10.745,325 us` | `23.182,605 us` |
| Sensor Android → preparação de envio | `5.133 us` | `20.576 us` | `26.251 us` |
| Preparação de envio → ingresso desktop, estimado | `2.714,422 us` | `14.624,572 us` | `50.640,733 us` |
| Ingresso desktop → tilt processado | `159 us` | `194 us` | `229 us` |
| Sensor → ingresso desktop, estimado | `8.331,207 us` | `29.981,800 us` | `59.297,898 us` |
| Sensor → tilt processado, estimado | `8.496,018 us` | `30.183,800 us` | `59.468,898 us` |

A mudança observada de offset foi `-1.339 us`, a distância máxima até uma âncora foi `605.495,5 us` e a incerteza de sincronização teve `p95 = 10.745,325 us`. Esses números não são thresholds nem veredito de desempenho. Em particular, estimativas entre relógios podem assumir valores negativos dentro do intervalo de incerteza; `RTT/2` depende da hipótese de simetria. O ensaio não mede sensor → tela, sensor-to-photon, renderização, resposta visual completa ou efeito clínico.

Na UI Android, durante a janela observada, não houve backpressure, rejeições ou erros de envio. A leitura feita somente após o encerramento do receiver e o fechamento manual do sender mostrou `4.682` oferecidas e `4.681` enviadas, com zero `droppedBackpressure`, zero `rejectedSamples` e zero `sendErrors`: havia uma amostra pendente no fechamento. Esses totais incluem o período posterior à janela e não devem ser comparados diretamente às `3.601` amostras do relatório.

Os hashes SHA-256 dos artefatos aprovados são:

- dataset: `65F1F0F0858E44B965963B32576F11BF78AB72985E3F31F9D6E1998F98BB7353`;
- relatório: `0302FD8930ADABC5B69CADE46617455AFB43526B199FB250D41765B3B7894B34`.

Uma primeira execução, preservada em `20261022T124239Z`, reprovou apenas `dataset.summary_consistent`: `3.745` datagramas declarados contra `3.723` reconstruídos. O ensaio revelou que amostras recebidas após a janela nominal eram contadas operacionalmente, mas não geravam fato no dataset. A CLI passou a registrar amostras antes da primeira âncora e após a janela como rejeições auditáveis, e o loader passou a incluí-las na reconstrução de datagramas sem classificá-las como pacotes inválidos. A segunda execução, descrita acima, confirmou a correção.
