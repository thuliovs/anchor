# Motion Filter Selection V2

A V2 do contrato B3b mantém a metodologia offline da V1, mas adiciona vínculo explícito entre política e perfil B2 por fingerprint.

O fingerprint é implementado em `calibration::provenance` para ser compartilhado por seleção offline B3b e runtime B4a/B4b sem dependência da seleção sobre o núcleo vivo.

## Diferenças em relação à V1

- `selectionReportVersion = 2` para novos relatórios;
- a política emitida é `TiltEstimatorPolicyV2`;
- `inputs.calibrationProfileFingerprint` registra o perfil realmente usado;
- `recommendation.policy.calibrationProfileFingerprint` repete o mesmo vínculo;
- `TiltEstimatorPolicyV1` permanece representável como histórico, mas é inadequada para ativação viva.

Formato conceitual da política:

```json
{
  "version": 2,
  "candidate": "gravity_no_additional_anchor_filter",
  "parameters": {},
  "yawAvailable": false,
  "source": "b3b_offline_selection",
  "calibrationProfileFingerprint": {
    "version": 1,
    "algorithm": "sha256",
    "digest": "<64 hex lowercase>"
  }
}
```

## Compatibilidade

Relatórios V1 continuam sendo evidência histórica da seleção offline já feita. Eles não possuem vínculo suficiente para ativar o núcleo vivo B4a e não devem ser migrados silenciosamente anexando um perfil arbitrário.

## Evidência física V2

Em 1 de outubro de 2026, os nove cenários físicos B1 foram regenerados localmente com o mesmo celular e a convenção `flat_screen_up_portrait_top_toward_vehicle_front`. As capturas selecionadas ficaram próximas de `60 Hz`, tiveram uma única sessão por captura e não tiveram descartes do gravador. O perfil estacionário B2 passou no gate de qualidade sem violações.

A seleção física B3b V2 foi então executada com os nove cenários. Ela escolheu `gravity_no_additional_anchor_filter`, sem parâmetros adicionais, com `yawAvailable = false` e fingerprint do perfil B2 `43e9378ae7290c57bd4dbc6b8a2259a3126a4f0159077e268fa0ec2fa07d5797`.

Essa seleção é evidência operacional baseada em proxies físicos; não demonstra acurácia angular real. Os artefatos físicos continuam locais/ignorados pelo Git, portanto testes que dependem deles seguem ignorados quando não estão presentes no checkout.
