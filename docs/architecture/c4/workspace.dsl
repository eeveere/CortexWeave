workspace "CortexWeave v0.5 C4 Model" "Repository-grounded C4/Structurizr model for CortexWeave v0.5.0 as implemented." {
    !identifiers hierarchical

    model {
        !include model/systems.dsl
        !include model/containers.dsl
        !include model/deployment.dsl
        !include model/relationships.dsl
    }

    views {
        !include views/system-context.dsl
        !include views/containers.dsl
        !include views/application-services.dsl
        !include views/indexing.dsl
        !include views/retrieval-context.dsl
        !include views/structural-intelligence.dsl
        !include views/verified-experience.dsl
        !include views/dynamics.dsl
        !include styles.dsl
    }
}
