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

import io.netty.util.internal.ObjectUtil;

/**
 * Configuration for HTTP
 */
public final class HTTPConfiguration {
    private long maxContentLength;
    private int h2InitialWindowSize;
    private long h2MaxConcurrentStreams;
    private long h2MaxHeaderSizeList;
    private long h2MaxHeaderTableSize;
    private int h2MaxFrameSize;
    private int maxInitialLineLength;
    private int maxHeaderSize;
    private int maxChunkSize;
    private int compressionThreshold;
    private int deflateCompressionLevel;
    private int brotliCompressionLevel;

    HTTPConfiguration() {
        // Prevent outside initialization
    }

    public long maxContentLength() {
        return maxContentLength;
    }

    public HTTPConfiguration maxContentLength(long maxContentLength) {
        this.maxContentLength = ObjectUtil.checkPositive(maxContentLength, "maxContentLength");
        return this;
    }

    public int h2InitialWindowSize() {
        return h2InitialWindowSize;
    }

    public HTTPConfiguration setH2InitialWindowSize(int h2InitialWindowSize) {
        this.h2InitialWindowSize = ObjectUtil.checkPositive(h2InitialWindowSize, "h2InitialWindowSize");
        return this;
    }

    public long h2MaxConcurrentStreams() {
        return h2MaxConcurrentStreams;
    }

    public HTTPConfiguration h2MaxConcurrentStreams(long h2MaxConcurrentStreams) {
        this.h2MaxConcurrentStreams = ObjectUtil.checkPositive(h2MaxConcurrentStreams, "h2MaxConcurrentStreams");
        return this;
    }

    public long h2MaxHeaderSizeList() {
        return h2MaxHeaderSizeList;
    }

    public HTTPConfiguration h2MaxHeaderSizeList(long h2MaxHeaderSizeList) {
        this.h2MaxHeaderSizeList = ObjectUtil.checkPositive(h2MaxHeaderSizeList, "h2MaxHeaderSizeList");
        return this;
    }

    public long h2MaxHeaderTableSize() {
        return h2MaxHeaderTableSize;
    }

    public HTTPConfiguration h2MaxHeaderTableSize(long h2MaxHeaderTableSize) {
        this.h2MaxHeaderTableSize = ObjectUtil.checkPositive(h2MaxHeaderTableSize, "h2MaxHeaderTableSize");
        return this;
    }

    public int h2MaxFrameSize() {
        return h2MaxFrameSize;
    }

    public HTTPConfiguration h2MaxFrameSize(int h2MaxFrameSize) {
        this.h2MaxFrameSize = ObjectUtil.checkPositive(h2MaxFrameSize, "h2MaxFrameSize");
        return this;
    }

    public int maxInitialLineLength() {
        return maxInitialLineLength;
    }

    public HTTPConfiguration maxInitialLineLength(int maxInitialLineLength) {
        this.maxInitialLineLength = ObjectUtil.checkPositive(maxInitialLineLength, "maxInitialLineLength");
        return this;
    }

    public int maxHeaderSize() {
        return maxHeaderSize;
    }

    public HTTPConfiguration setMaxHeaderSize(int maxHeaderSize) {
        this.maxHeaderSize = ObjectUtil.checkPositive(maxHeaderSize, "maxHeaderSize");
        return this;
    }

    public int maxChunkSize() {
        return maxChunkSize;
    }

    public HTTPConfiguration maxChunkSize(int maxChunkSize) {
        this.maxChunkSize = ObjectUtil.checkPositive(maxChunkSize, "maxChunkSize");
        return this;
    }

    public int compressionThreshold() {
        return compressionThreshold;
    }

    public HTTPConfiguration compressionThreshold(int compressionThreshold) {
        this.compressionThreshold = ObjectUtil.checkPositiveOrZero(compressionThreshold, "compressionThreshold");
        return this;
    }

    public int deflateCompressionLevel() {
        return deflateCompressionLevel;
    }

    public HTTPConfiguration deflateCompressionLevel(int deflateCompressionLevel) {
        this.deflateCompressionLevel = ObjectUtil.checkInRange(deflateCompressionLevel, 0, 9, "deflateCompressionLevel");
        return this;
    }

    public int brotliCompressionLevel() {
        return brotliCompressionLevel;
    }

    public HTTPConfiguration brotliCompressionLevel(int brotliCompressionLevel) {
        this.brotliCompressionLevel = ObjectUtil.checkInRange(brotliCompressionLevel, 1, 11, "brotliCompressionLevel");
        return this;
    }
}
