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
package com.shieldblaze.expressgateway.bootstrap;

import com.google.gson.JsonNull;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import static com.shieldblaze.expressgateway.bootstrap.Utils.*;
import static com.shieldblaze.expressgateway.common.utils.StringUtil.EMPTY_STRING;
import static org.junit.jupiter.api.Assertions.*;

class UtilsTest {

    @Test
    void validateEnforcingTest() {
        assertThrows(IllegalArgumentException.class, () -> validateEnforcing(true, "Test"));
        assertDoesNotThrow(() -> validateEnforcing(false, "Test"));
    }

    @Test
    void validateStringNullOrEmptyEnvTest() {
        assertThrows(IllegalArgumentException.class, () -> validateStringNullOrEmptyEnv(null, "Test"));
        assertThrows(IllegalArgumentException.class, () -> validateStringNullOrEmptyEnv(EMPTY_STRING, "Test"));
        assertThrows(IllegalArgumentException.class, () -> validateStringNullOrEmptyEnv(" ", "Test"));

        assertDoesNotThrow(() -> validateStringNullOrEmptyEnv(".", "Test"));
        assertDoesNotThrow(() -> validateStringNullOrEmptyEnv("@", "Test"));
        assertDoesNotThrow(() -> validateStringNullOrEmptyEnv("Test", "Test"));
    }

    @Test
    void validateNullTest() {
        assertThrows(NullPointerException.class, () -> validateNullEnv(null, "Test"));

        assertDoesNotThrow(() -> validateNullEnv(new Object(), "Test"));
    }

    @Test
    void validateJsonElementNullTest() {
        JsonObject jsonObject = new JsonObject();
        jsonObject.addProperty("Name", "ShieldBlaze");

        assertThrows(NullPointerException.class, () -> validateJsonElementNullConf(jsonObject.get("ShieldBlaze"), "Test"));
        assertThrows(NullPointerException.class, () -> validateJsonElementNullConf(JsonNull.INSTANCE, "Test"));

        assertDoesNotThrow(() -> validateJsonElementNullConf(jsonObject.get("Name"), "Test"));
    }

    @Test
    void validateStringNullOrEmptyConfTest() {
        JsonObject jsonObject = new JsonObject();
        jsonObject.addProperty("Name", "ShieldBlaze");
        jsonObject.addProperty("Running", 1);

        assertThrows(NullPointerException.class, () -> validateStringNullOrEmptyConf(JsonNull.INSTANCE, "Test"));
        assertThrows(NullPointerException.class, () -> validateStringNullOrEmptyConf(jsonObject.get("ShieldBlaze"), "Test"));
        assertThrows(NullPointerException.class, () -> validateStringNullOrEmptyConf(null, "Test"));

        assertDoesNotThrow(() -> validateStringNullOrEmptyConf(jsonObject.get("Name"), "Test"));
        assertDoesNotThrow(() -> validateStringNullOrEmptyConf(jsonObject.get("Running"), "Test"));
    }

    @Test
    void checkStringNullOrEmptyTest() {
        assertTrue(checkStringNullOrEmpty(null));
        assertTrue(checkStringNullOrEmpty(JsonNull.INSTANCE));

        JsonObject jsonObject = new JsonObject();
        jsonObject.addProperty("Name", "ShieldBlaze");

        assertFalse(checkStringNullOrEmpty(jsonObject.get("Name")));
    }
}
