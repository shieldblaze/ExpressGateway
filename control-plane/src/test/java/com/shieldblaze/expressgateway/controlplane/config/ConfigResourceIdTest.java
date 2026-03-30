package com.shieldblaze.expressgateway.controlplane.config;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class ConfigResourceIdTest {

    @Test
    void validConstructionAndFieldAccess() {
        ConfigResourceId id = new ConfigResourceId("cluster", "global", "prod-1");
        assertEquals("cluster", id.kind());
        assertEquals("global", id.scopeQualifier());
        assertEquals("prod-1", id.name());
    }

    @Test
    void toPathReturnsKindScopeQualifierName() {
        ConfigResourceId id = new ConfigResourceId("listener", "cluster:west", "https-443");
        assertEquals("listener/cluster:west/https-443", id.toPath());
    }

    @Test
    void fromPathParsesCorrectly() {
        ConfigResourceId id = ConfigResourceId.fromPath("routing-rule/node:dp-001/default");
        assertEquals("routing-rule", id.kind());
        assertEquals("node:dp-001", id.scopeQualifier());
        assertEquals("default", id.name());
    }

    @Test
    void fromPathRoundTrip() {
        ConfigResourceId original = new ConfigResourceId("cluster", "global", "my-resource");
        ConfigResourceId parsed = ConfigResourceId.fromPath(original.toPath());
        assertEquals(original, parsed);
    }

    @Test
    void fromPathRejectsInvalidPaths() {
        assertThrows(IllegalArgumentException.class, () -> ConfigResourceId.fromPath("onlyone"));
        assertThrows(IllegalArgumentException.class, () -> ConfigResourceId.fromPath("only/two"));
    }

    @Test
    void fromPathRejectsNull() {
        assertThrows(NullPointerException.class, () -> ConfigResourceId.fromPath(null));
    }

    @Test
    void nullKindRejected() {
        assertThrows(NullPointerException.class,
                () -> new ConfigResourceId(null, "global", "name"));
    }

    @Test
    void nullScopeQualifierRejected() {
        assertThrows(NullPointerException.class,
                () -> new ConfigResourceId("cluster", null, "name"));
    }

    @Test
    void nullNameRejected() {
        assertThrows(NullPointerException.class,
                () -> new ConfigResourceId("cluster", "global", null));
    }

    @Test
    void blankKindRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigResourceId("", "global", "name"));
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigResourceId("   ", "global", "name"));
    }

    @Test
    void blankScopeQualifierRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigResourceId("cluster", "", "name"));
    }

    @Test
    void blankNameRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigResourceId("cluster", "global", ""));
    }

    @Test
    void slashInKindRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigResourceId("cl/uster", "global", "name"));
    }

    @Test
    void slashInScopeQualifierRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigResourceId("cluster", "glo/bal", "name"));
    }

    @Test
    void slashInNameRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigResourceId("cluster", "global", "na/me"));
    }

    @Test
    void nullCharInKindRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigResourceId("cl\0uster", "global", "name"));
    }

    @Test
    void nullCharInScopeQualifierRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigResourceId("cluster", "glo\0bal", "name"));
    }

    @Test
    void nullCharInNameRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigResourceId("cluster", "global", "na\0me"));
    }

    @Test
    void equalsAndHashCode() {
        ConfigResourceId a = new ConfigResourceId("cluster", "global", "prod");
        ConfigResourceId b = new ConfigResourceId("cluster", "global", "prod");
        ConfigResourceId c = new ConfigResourceId("cluster", "global", "staging");

        assertEquals(a, b);
        assertEquals(a.hashCode(), b.hashCode());
        assertNotEquals(a, c);
    }
}
