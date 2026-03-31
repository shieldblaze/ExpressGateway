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
package com.shieldblaze.expressgateway.testing;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import org.junit.jupiter.api.Test;

import java.nio.charset.StandardCharsets;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;

class ByteBufAssertTest {

    @Test
    void readableBuffer() {
        ByteBuf buf = Unpooled.copiedBuffer("hello world", StandardCharsets.UTF_8);
        try {
            assertDoesNotThrow(() -> ByteBufAssert.assertThat(buf)
                    .isReadable()
                    .hasReadableBytes(11)
                    .hasMinReadableBytes(5)
                    .containsUtf8("hello")
                    .containsUtf8("world")
                    .equalsUtf8("hello world")
                    .hasRefCnt(1));
        } finally {
            buf.release();
        }
    }

    @Test
    void emptyBuffer() {
        ByteBuf buf = Unpooled.EMPTY_BUFFER;
        assertDoesNotThrow(() -> ByteBufAssert.assertThat(buf)
                .isNotReadable()
                .hasReadableBytes(0));
    }

    @Test
    void wrongReadableBytesThrows() {
        ByteBuf buf = Unpooled.copiedBuffer("test", StandardCharsets.UTF_8);
        try {
            assertThrows(AssertionError.class, () -> ByteBufAssert.assertThat(buf).hasReadableBytes(99));
        } finally {
            buf.release();
        }
    }

    @Test
    void wrongContentThrows() {
        ByteBuf buf = Unpooled.copiedBuffer("abc", StandardCharsets.UTF_8);
        try {
            assertThrows(AssertionError.class, () -> ByteBufAssert.assertThat(buf).containsUtf8("xyz"));
        } finally {
            buf.release();
        }
    }

    @Test
    void equalsBytesMatches() {
        byte[] data = {0x01, 0x02, 0x03};
        ByteBuf buf = Unpooled.wrappedBuffer(data);
        try {
            assertDoesNotThrow(() -> ByteBufAssert.assertThat(buf).equalsBytes(data));
        } finally {
            buf.release();
        }
    }

    @Test
    void equalsBytesMismatchThrows() {
        byte[] data = {0x01, 0x02, 0x03};
        ByteBuf buf = Unpooled.wrappedBuffer(data);
        try {
            assertThrows(AssertionError.class,
                    () -> ByteBufAssert.assertThat(buf).equalsBytes(new byte[]{0x01, 0x02, 0x04}));
        } finally {
            buf.release();
        }
    }

    @Test
    void releasedBuffer() {
        ByteBuf buf = Unpooled.buffer(10);
        buf.release();
        assertDoesNotThrow(() -> ByteBufAssert.assertThat(buf).isReleased());
    }

    @Test
    void minCapacity() {
        ByteBuf buf = Unpooled.buffer(256);
        try {
            assertDoesNotThrow(() -> ByteBufAssert.assertThat(buf).hasMinCapacity(256));
            assertThrows(AssertionError.class, () -> ByteBufAssert.assertThat(buf).hasMinCapacity(1024));
        } finally {
            buf.release();
        }
    }
}
