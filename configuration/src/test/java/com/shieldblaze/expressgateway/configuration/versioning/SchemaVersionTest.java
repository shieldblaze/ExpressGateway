/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.configuration.versioning;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class SchemaVersionTest {

    @Test
    void validVersionCreation() {
        var v = new SchemaVersion(1, 2, 3);
        assertEquals(1, v.major());
        assertEquals(2, v.minor());
        assertEquals(3, v.patch());
    }

    @Test
    void v100Constant() {
        assertEquals(new SchemaVersion(1, 0, 0), SchemaVersion.V1_0_0);
    }

    @Test
    void negativeMajorRejected() {
        assertThrows(IllegalArgumentException.class, () -> new SchemaVersion(-1, 0, 0));
    }

    @Test
    void negativeMinorRejected() {
        assertThrows(IllegalArgumentException.class, () -> new SchemaVersion(0, -1, 0));
    }

    @Test
    void negativePatchRejected() {
        assertThrows(IllegalArgumentException.class, () -> new SchemaVersion(0, 0, -1));
    }

    @Test
    void parseValid() {
        assertEquals(new SchemaVersion(1, 2, 3), SchemaVersion.parse("1.2.3"));
        assertEquals(new SchemaVersion(0, 0, 0), SchemaVersion.parse("0.0.0"));
        assertEquals(new SchemaVersion(10, 20, 30), SchemaVersion.parse("10.20.30"));
    }

    @Test
    void parseInvalidFormatRejected() {
        assertThrows(IllegalArgumentException.class, () -> SchemaVersion.parse("1.2"));
        assertThrows(IllegalArgumentException.class, () -> SchemaVersion.parse("1"));
        assertThrows(IllegalArgumentException.class, () -> SchemaVersion.parse("1.2.3.4"));
    }

    @Test
    void parseNonNumericRejected() {
        assertThrows(IllegalArgumentException.class, () -> SchemaVersion.parse("a.b.c"));
    }

    @Test
    void parseNullRejected() {
        assertThrows(NullPointerException.class, () -> SchemaVersion.parse(null));
    }

    @Test
    void toStringFormat() {
        assertEquals("1.2.3", new SchemaVersion(1, 2, 3).toString());
        assertEquals("0.0.0", new SchemaVersion(0, 0, 0).toString());
    }

    @Test
    void compareToOrdering() {
        var v100 = new SchemaVersion(1, 0, 0);
        var v110 = new SchemaVersion(1, 1, 0);
        var v111 = new SchemaVersion(1, 1, 1);
        var v200 = new SchemaVersion(2, 0, 0);

        assertTrue(v100.compareTo(v110) < 0);
        assertTrue(v110.compareTo(v111) < 0);
        assertTrue(v111.compareTo(v200) < 0);
        assertTrue(v200.compareTo(v100) > 0);
        assertEquals(0, v100.compareTo(new SchemaVersion(1, 0, 0)));
    }

    @Test
    void compatibilityChecks() {
        var v100 = new SchemaVersion(1, 0, 0);
        var v110 = new SchemaVersion(1, 1, 0);
        var v200 = new SchemaVersion(2, 0, 0);

        // v1.1.0 can read v1.0.0 (same major, higher minor)
        assertTrue(v110.isCompatibleWith(v100));
        // v1.0.0 is compatible with itself
        assertTrue(v100.isCompatibleWith(v100));
        // v1.0.0 cannot read v1.1.0 (older minor)
        assertFalse(v100.isCompatibleWith(v110));
        // v2.0.0 cannot read v1.0.0 (different major)
        assertFalse(v200.isCompatibleWith(v100));
    }

    @Test
    void breakingUpgradeCheck() {
        var v100 = new SchemaVersion(1, 0, 0);
        var v110 = new SchemaVersion(1, 1, 0);
        var v200 = new SchemaVersion(2, 0, 0);

        assertFalse(v110.isBreakingUpgradeFrom(v100)); // Same major
        assertTrue(v200.isBreakingUpgradeFrom(v100));   // Different major
        assertFalse(v100.isBreakingUpgradeFrom(v100));  // Same version
    }

    @Test
    void equalsAndHashCode() {
        var a = new SchemaVersion(1, 2, 3);
        var b = new SchemaVersion(1, 2, 3);
        var c = new SchemaVersion(1, 2, 4);

        assertEquals(a, b);
        assertEquals(a.hashCode(), b.hashCode());
        assertNotEquals(a, c);
    }
}
