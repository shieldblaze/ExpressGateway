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

import org.junit.jupiter.api.Test;

import static com.shieldblaze.expressgateway.bootstrap.Utils.*;
import static com.shieldblaze.expressgateway.common.utils.StringUtil.EMPTY_STRING;
import static org.junit.jupiter.api.Assertions.*;

class UtilsTest {

    @Test
    void validateEnforcingTest() {
        assertThrows(IllegalArgumentException.class, () -> validateEnforcing(true, "Demo"));
        assertDoesNotThrow(() -> validateEnforcing(false, "Demo"));
    }

    @Test
    void checkStringNullOrEmptyEnvTest() {
        assertThrows(IllegalArgumentException.class, () -> checkStringNullOrEmptyEnv(null, "Demo"));
        assertThrows(IllegalArgumentException.class, () -> checkStringNullOrEmptyEnv(EMPTY_STRING, "Demo"));
        assertThrows(IllegalArgumentException.class, () -> checkStringNullOrEmptyEnv(" ", "Demo"));

        assertDoesNotThrow(() -> checkStringNullOrEmptyEnv(".", "Demo"));
        assertDoesNotThrow(() -> checkStringNullOrEmptyEnv("@", "Demo"));
        assertDoesNotThrow(() -> checkStringNullOrEmptyEnv("Test", "Demo"));
    }

    @Test
    void testCheckStringNullOrEmptyEnv() {
    }

    @Test
    void checkNullEnv() {
    }

    @Test
    void checkJsonElementNull() {
    }

    @Test
    void checkStringNullOrEmptyConf() {
    }

    @Test
    void testCheckStringNullOrEmptyConf() {
    }
}
