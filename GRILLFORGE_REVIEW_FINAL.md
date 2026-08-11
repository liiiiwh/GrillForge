# Final GrillForge Review

Changes from previous version:

1. Removed Claude Default from model registry.
   - Claude native behavior should never be managed by GrillForge.

2. Removed fixed modes.
   - Mixed behavior is naturally determined by enabled external model pool.

3. Added enable/disable model switch.
   - Any configured model can be enabled.
   - At least one model remains enabled when external mode is active.

4. Clarified cc-switch reuse requirement.
   - Provider compatibility should be reused/adapted.

5. Reduced implementation scope.
   - No Agent Runtime.
   - No MCP.
   - No workflow system.
