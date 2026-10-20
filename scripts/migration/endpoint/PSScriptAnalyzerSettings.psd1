@{
    Severity     = @('Error', 'Warning')
    ExcludeRules = @(
        # Cleanup, probing and best-effort reads are meant to carry on when they fail: the outcome is
        # reported in status.json or the result, and an error here must not stop the install.
        'PSAvoidUsingEmptyCatchBlock',
        # Internal helpers in single-purpose scripts that run unattended as SYSTEM; there is no -WhatIf caller.
        'PSUseShouldProcessForStateChangingFunctions'
    )
}
