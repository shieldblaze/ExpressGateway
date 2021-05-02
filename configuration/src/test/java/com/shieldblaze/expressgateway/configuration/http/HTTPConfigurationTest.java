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
package com.shieldblaze.expressgateway.configuration.http;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

class HTTPConfigurationTest {

    @Test
    void maxContentLengthTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().maxContentLength(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().maxContentLength(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().maxContentLength(Integer.MIN_VALUE));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().maxContentLength(Long.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().maxContentLength(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().maxContentLength(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().maxContentLength(Integer.MAX_VALUE));
        assertDoesNotThrow(() -> new HTTPConfiguration().maxContentLength(Long.MAX_VALUE));
    }

    @Test
    void h2InitialWindowSizeTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2InitialWindowSize(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2InitialWindowSize(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2InitialWindowSize(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().h2InitialWindowSize(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().h2InitialWindowSize(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().h2InitialWindowSize(Integer.MAX_VALUE));
    }

    @Test
    void h2MaxConcurrentStreamsTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2MaxConcurrentStreams(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2MaxConcurrentStreams(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2MaxConcurrentStreams(Integer.MIN_VALUE));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2MaxConcurrentStreams(Long.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().h2MaxConcurrentStreams(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().h2MaxConcurrentStreams(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().h2MaxConcurrentStreams(Integer.MAX_VALUE));
        assertDoesNotThrow(() -> new HTTPConfiguration().h2MaxConcurrentStreams(Long.MAX_VALUE));
    }

    @Test
    void h2MaxHeaderListSizeTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2MaxHeaderListSize(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2MaxHeaderListSize(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2MaxHeaderListSize(Integer.MIN_VALUE));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2MaxHeaderListSize(Long.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().h2MaxHeaderListSize(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().h2MaxHeaderListSize(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().h2MaxHeaderListSize(Integer.MAX_VALUE));
        assertDoesNotThrow(() -> new HTTPConfiguration().h2MaxHeaderListSize(Long.MAX_VALUE));
    }

    @Test
    void h2MaxHeaderTableSizeTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2MaxHeaderTableSize(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2MaxHeaderTableSize(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2MaxHeaderTableSize(Integer.MIN_VALUE));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2MaxHeaderTableSize(Long.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().h2MaxHeaderTableSize(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().h2MaxHeaderTableSize(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().h2MaxHeaderTableSize(Integer.MAX_VALUE));
        assertDoesNotThrow(() -> new HTTPConfiguration().h2MaxHeaderTableSize(Long.MAX_VALUE));
    }

    @Test
    void h2MaxFrameSizeTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2MaxFrameSize(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2MaxFrameSize(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().h2MaxFrameSize(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().h2MaxFrameSize(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().h2MaxFrameSize(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().h2MaxFrameSize(Integer.MAX_VALUE));
    }

    @Test
    void maxInitialLineLengthTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().maxInitialLineLength(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().maxInitialLineLength(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().maxInitialLineLength(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().maxInitialLineLength(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().maxInitialLineLength(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().maxInitialLineLength(Integer.MAX_VALUE));
    }

    @Test
    void maxHeaderSizeTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().maxHeaderSize(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().maxHeaderSize(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().maxHeaderSize(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().maxHeaderSize(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().maxHeaderSize(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().maxHeaderSize(Integer.MAX_VALUE));
    }

    @Test
    void maxChunkSizeTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().maxChunkSize(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().maxChunkSize(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().maxChunkSize(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().maxChunkSize(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().maxChunkSize(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().maxChunkSize(Integer.MAX_VALUE));
    }

    @Test
    void compressionThresholdTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().compressionThreshold(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().compressionThreshold(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().compressionThreshold(0));
        assertDoesNotThrow(() -> new HTTPConfiguration().compressionThreshold(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().compressionThreshold(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().compressionThreshold(Integer.MAX_VALUE));
    }

    @Test
    void deflateCompressionLevelTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().deflateCompressionLevel(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().deflateCompressionLevel(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().deflateCompressionLevel(0));
        assertDoesNotThrow(() -> new HTTPConfiguration().deflateCompressionLevel(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().deflateCompressionLevel(9));
    }

    @Test
    void brotliCompressionLevelTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().brotliCompressionLevel(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().brotliCompressionLevel(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().brotliCompressionLevel(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().brotliCompressionLevel(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().brotliCompressionLevel(8));
    }
}
