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
    void checkStringNullOrEmptyEnvTest() {
        assertThrows(IllegalArgumentException.class, () -> checkStringNullOrEmptyEnv(null, "Test"));
        assertThrows(IllegalArgumentException.class, () -> checkStringNullOrEmptyEnv(EMPTY_STRING, "Test"));
        assertThrows(IllegalArgumentException.class, () -> checkStringNullOrEmptyEnv(" ", "Test"));

        assertDoesNotThrow(() -> checkStringNullOrEmptyEnv(".", "Test"));
        assertDoesNotThrow(() -> checkStringNullOrEmptyEnv("@", "Test"));
        assertDoesNotThrow(() -> checkStringNullOrEmptyEnv("Test", "Test"));
    }

    @Test
    void checkNullTest() {
        assertThrows(NullPointerException.class, () -> checkNullEnv(null, "Test"));

        assertDoesNotThrow(() -> checkNullEnv(new Object(), "Test"));
    }

    @Test
    void checkJsonElementNullTest() {
        JsonObject jsonObject = new JsonObject();
        jsonObject.addProperty("Name", "ShieldBlaze");

        assertThrows(NullPointerException.class, () -> checkJsonElementNullConf(jsonObject.get("ShieldBlaze"), "Test"));
        assertThrows(NullPointerException.class, () -> checkJsonElementNullConf(JsonNull.INSTANCE, "Test"));

        assertDoesNotThrow(() -> checkJsonElementNullConf(jsonObject.get("Name"), "Test"));
    }

    @Test
    void checkStringNullOrEmptyConfTest() {
        JsonObject jsonObject = new JsonObject();
        jsonObject.addProperty("Name", "ShieldBlaze");
        jsonObject.addProperty("Running", 1);

        assertThrows(NullPointerException.class, () -> checkStringNullOrEmptyConf(JsonNull.INSTANCE, "Test"));
        assertThrows(NullPointerException.class, () -> checkStringNullOrEmptyConf(jsonObject.get("ShieldBlaze"), "Test"));
        assertThrows(NullPointerException.class, () -> checkStringNullOrEmptyConf(null, "Test"));

        assertDoesNotThrow(() -> checkStringNullOrEmptyConf(jsonObject.get("Name"), "Test"));
        assertDoesNotThrow(() -> checkStringNullOrEmptyConf(jsonObject.get("Running"), "Test"));
    }
}
