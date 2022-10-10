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
package com.shieldblaze.expressgateway.common.utils;

import org.junit.jupiter.api.Test;

import static com.shieldblaze.expressgateway.common.utils.StringUtil.EMPTY_STRING;
import static com.shieldblaze.expressgateway.common.utils.StringUtil.isNullOrEmpty;
import static com.shieldblaze.expressgateway.common.utils.StringUtil.validateNotNullOrEmpty;
import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class StringUtilTest {

    @Test
    void checkNullOrEmptyTest() {
        assertTrue(isNullOrEmpty(EMPTY_STRING));
        assertTrue(isNullOrEmpty(null));
        assertTrue(isNullOrEmpty(" "));

        assertFalse(isNullOrEmpty("Test"));
        assertFalse(isNullOrEmpty("@"));
        assertFalse(isNullOrEmpty("."));
    }

    @Test
    void validateNotNullOrEmptyTest() {
        assertThrows(NullPointerException.class,  () -> validateNotNullOrEmpty(null));
        assertThrows(IllegalArgumentException.class,  () -> validateNotNullOrEmpty(EMPTY_STRING));
        assertThrows(IllegalArgumentException.class, () -> validateNotNullOrEmpty(" "));

        assertDoesNotThrow(() -> validateNotNullOrEmpty("."));
        assertDoesNotThrow(() -> validateNotNullOrEmpty("Test"));
        assertDoesNotThrow(() -> validateNotNullOrEmpty("@"));
    }
}
