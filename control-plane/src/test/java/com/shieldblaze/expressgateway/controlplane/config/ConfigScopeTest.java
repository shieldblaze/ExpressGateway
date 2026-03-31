package com.shieldblaze.expressgateway.controlplane.config;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertThrows;

class ConfigScopeTest {

    @Test
    void globalQualifierReturnsGlobal() {
        ConfigScope scope = new ConfigScope.Global();
        assertEquals("global", scope.qualifier());
    }

    @Test
    void clusterScopedQualifierReturnsClusterColonId() {
        ConfigScope scope = new ConfigScope.ClusterScoped("prod-1");
        assertEquals("cluster:prod-1", scope.qualifier());
    }

    @Test
    void nodeScopedQualifierReturnsNodeColonId() {
        ConfigScope scope = new ConfigScope.NodeScoped("dp-001");
        assertEquals("node:dp-001", scope.qualifier());
    }

    @Test
    void fromQualifierGlobal() {
        ConfigScope scope = ConfigScope.fromQualifier("global");
        assertInstanceOf(ConfigScope.Global.class, scope);
        assertEquals("global", scope.qualifier());
    }

    @Test
    void fromQualifierClusterScoped() {
        ConfigScope scope = ConfigScope.fromQualifier("cluster:prod-1");
        assertInstanceOf(ConfigScope.ClusterScoped.class, scope);
        assertEquals("prod-1", ((ConfigScope.ClusterScoped) scope).clusterId());
    }

    @Test
    void fromQualifierNodeScoped() {
        ConfigScope scope = ConfigScope.fromQualifier("node:dp-001");
        assertInstanceOf(ConfigScope.NodeScoped.class, scope);
        assertEquals("dp-001", ((ConfigScope.NodeScoped) scope).nodeId());
    }

    @Test
    void fromQualifierUnknownFormatThrows() {
        assertThrows(IllegalArgumentException.class,
                () -> ConfigScope.fromQualifier("unknown:xyz"));
        assertThrows(IllegalArgumentException.class,
                () -> ConfigScope.fromQualifier("something-else"));
    }

    @Test
    void fromQualifierClusterWithBlankIdThrows() {
        assertThrows(IllegalArgumentException.class,
                () -> ConfigScope.fromQualifier("cluster:"));
        assertThrows(IllegalArgumentException.class,
                () -> ConfigScope.fromQualifier("cluster:   "));
    }

    @Test
    void fromQualifierNodeWithBlankIdThrows() {
        assertThrows(IllegalArgumentException.class,
                () -> ConfigScope.fromQualifier("node:"));
        assertThrows(IllegalArgumentException.class,
                () -> ConfigScope.fromQualifier("node:   "));
    }

    @Test
    void clusterScopedBlankClusterIdThrows() {
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigScope.ClusterScoped(""));
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigScope.ClusterScoped("   "));
    }

    @Test
    void nodeScopedBlankNodeIdThrows() {
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigScope.NodeScoped(""));
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigScope.NodeScoped("   "));
    }

    @Test
    void clusterScopedNullClusterIdThrows() {
        assertThrows(NullPointerException.class,
                () -> new ConfigScope.ClusterScoped(null));
    }

    @Test
    void nodeScopedNullNodeIdThrows() {
        assertThrows(NullPointerException.class,
                () -> new ConfigScope.NodeScoped(null));
    }

    @Test
    void fromQualifierNullThrows() {
        assertThrows(NullPointerException.class,
                () -> ConfigScope.fromQualifier(null));
    }
}
