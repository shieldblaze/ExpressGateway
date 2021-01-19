/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;

class NumberTest {

    @Test
    void testCheckZeroOrPositive() {
        assertDoesNotThrow(() -> Number.checkZeroOrPositive(0, null));
        assertDoesNotThrow(() -> Number.checkZeroOrPositive(1, null));
        assertDoesNotThrow(() -> Number.checkZeroOrPositive(Integer.MAX_VALUE, null));

        assertDoesNotThrow(() -> Number.checkZeroOrPositive(0L, null));
        assertDoesNotThrow(() -> Number.checkZeroOrPositive(1L, null));
        assertDoesNotThrow(() -> Number.checkZeroOrPositive(Long.MAX_VALUE, null));

        assertThrows(IllegalArgumentException.class, () -> Number.checkZeroOrPositive(-1, null));
        assertThrows(IllegalArgumentException.class, () -> Number.checkZeroOrPositive(-82, null));
        assertThrows(IllegalArgumentException.class, () -> Number.checkZeroOrPositive(Integer.MIN_VALUE, null));

        assertThrows(IllegalArgumentException.class, () -> Number.checkZeroOrPositive(-1L, null));
        assertThrows(IllegalArgumentException.class, () -> Number.checkZeroOrPositive(-82L, null));
        assertThrows(IllegalArgumentException.class, () -> Number.checkZeroOrPositive(Long.MIN_VALUE, null));
    }

    @Test
    void testCheckPositive() {
        assertDoesNotThrow(() -> Number.checkPositive(1, null));
        assertDoesNotThrow(() -> Number.checkPositive(54, null));
        assertDoesNotThrow(() -> Number.checkPositive(Integer.MAX_VALUE, null));

        assertDoesNotThrow(() -> Number.checkPositive(1L, null));
        assertDoesNotThrow(() -> Number.checkPositive(9110L, null));
        assertDoesNotThrow(() -> Number.checkPositive(Long.MAX_VALUE, null));

        assertThrows(IllegalArgumentException.class, () -> Number.checkPositive(0, null));
        assertThrows(IllegalArgumentException.class, () -> Number.checkPositive(-82, null));
        assertThrows(IllegalArgumentException.class, () -> Number.checkPositive(Integer.MIN_VALUE, null));

        assertThrows(IllegalArgumentException.class, () -> Number.checkPositive(-1L, null));
        assertThrows(IllegalArgumentException.class, () -> Number.checkPositive(-82L, null));
        assertThrows(IllegalArgumentException.class, () -> Number.checkPositive(Long.MIN_VALUE, null));
    }

    @Test
    void testCheckRange() {
        assertDoesNotThrow(() -> Number.checkRange(100, -1, 100, null));
        assertDoesNotThrow(() -> Number.checkRange(52, 52, 70, null));
        assertDoesNotThrow(() -> Number.checkRange(9110, Integer.MIN_VALUE, Integer.MAX_VALUE, null));

        assertDoesNotThrow(() -> Number.checkRange(100L, -1L, 100L, null));
        assertDoesNotThrow(() -> Number.checkRange(52L, 52L, 70L, null));
        assertDoesNotThrow(() -> Number.checkRange(9110L, Long.MIN_VALUE, Long.MAX_VALUE, null));

        assertThrows(IllegalArgumentException.class, () -> Number.checkRange(99, 100, 200, null));
        assertThrows(IllegalArgumentException.class, () -> Number.checkRange(201, 100, 200, null));
        assertThrows(IllegalArgumentException.class, () -> Number.checkRange(-1002, -1000, 900, null));

        assertThrows(IllegalArgumentException.class, () -> Number.checkRange(99L, 100L, 200L, null));
        assertThrows(IllegalArgumentException.class, () -> Number.checkRange(201L, 100L, 200L, null));
        assertThrows(IllegalArgumentException.class, () -> Number.checkRange(-1002L, -1000L, 900L, null));
    }
}
