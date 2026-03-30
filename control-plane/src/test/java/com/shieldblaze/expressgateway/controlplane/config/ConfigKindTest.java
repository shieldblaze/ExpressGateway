package com.shieldblaze.expressgateway.controlplane.config;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class ConfigKindTest {

    @Test
    void wellKnownClusterKind() {
        assertEquals("cluster", ConfigKind.CLUSTER.name());
        assertEquals(1, ConfigKind.CLUSTER.schemaVersion());
    }

    @Test
    void wellKnownListenerKind() {
        assertEquals("listener", ConfigKind.LISTENER.name());
        assertEquals(1, ConfigKind.LISTENER.schemaVersion());
    }

    @Test
    void wellKnownRoutingRuleKind() {
        assertEquals("routing-rule", ConfigKind.ROUTING_RULE.name());
        assertEquals(1, ConfigKind.ROUTING_RULE.schemaVersion());
    }

    @Test
    void wellKnownLbStrategyKind() {
        assertEquals("lb-strategy", ConfigKind.LB_STRATEGY.name());
        assertEquals(1, ConfigKind.LB_STRATEGY.schemaVersion());
    }

    @Test
    void wellKnownHealthCheckKind() {
        assertEquals("health-check", ConfigKind.HEALTH_CHECK.name());
        assertEquals(1, ConfigKind.HEALTH_CHECK.schemaVersion());
    }

    @Test
    void wellKnownTlsCertificateKind() {
        assertEquals("tls-certificate", ConfigKind.TLS_CERTIFICATE.name());
        assertEquals(1, ConfigKind.TLS_CERTIFICATE.schemaVersion());
    }

    @Test
    void customKindCanBeCreated() {
        ConfigKind custom = new ConfigKind("my-custom-kind", 3);
        assertEquals("my-custom-kind", custom.name());
        assertEquals(3, custom.schemaVersion());
    }

    @Test
    void blankNameRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigKind("", 1));
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigKind("   ", 1));
    }

    @Test
    void nullNameRejected() {
        assertThrows(NullPointerException.class,
                () -> new ConfigKind(null, 1));
    }

    @Test
    void schemaVersionBelowOneRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigKind("test", 0));
        assertThrows(IllegalArgumentException.class,
                () -> new ConfigKind("test", -1));
    }

    @Test
    void equalsAndHashCode() {
        ConfigKind a = new ConfigKind("cluster", 1);
        ConfigKind b = new ConfigKind("cluster", 1);
        ConfigKind c = new ConfigKind("cluster", 2);
        ConfigKind d = new ConfigKind("listener", 1);

        assertEquals(a, b);
        assertEquals(a.hashCode(), b.hashCode());
        assertNotEquals(a, c);
        assertNotEquals(a, d);
    }

    @Test
    void wellKnownKindsEqualToEquivalentNew() {
        assertEquals(ConfigKind.CLUSTER, new ConfigKind("cluster", 1));
    }
}
