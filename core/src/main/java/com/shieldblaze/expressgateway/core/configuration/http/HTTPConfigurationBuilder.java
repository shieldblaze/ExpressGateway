/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.core.configuration.http;

import io.netty.util.internal.ObjectUtil;

/**
 * Configuration Builder for {@link HTTPConfiguration}
 */
public final class HTTPConfigurationBuilder {
    private long maxContentLength;
    private int initialWindowSize;
    private int maxConcurrentStreams;
    private long maxHeaderSizeList;
    private int maxInitialLineLength;
    private int maxHeaderSize;
    private int maxChunkSize;
    private int compressionThreshold;
    private int deflateCompressionLevel;
    private int brotliCompressionLevel;
    private boolean enableHTTP2Push;

    private HTTPConfigurationBuilder() {
    }

    public static HTTPConfigurationBuilder newBuilder() {
        return new HTTPConfigurationBuilder();
    }

    public HTTPConfigurationBuilder withMaxContentLength(long maxContentLength) {
        this.maxContentLength = maxContentLength;
        return this;
    }

    public HTTPConfigurationBuilder withInitialWindowSize(int initialWindowSize) {
        this.initialWindowSize = initialWindowSize;
        return this;
    }

    public HTTPConfigurationBuilder withMaxConcurrentStreams(int maxConcurrentStreams) {
        this.maxConcurrentStreams = maxConcurrentStreams;
        return this;
    }

    public HTTPConfigurationBuilder withMaxHeaderSizeList(long maxHeaderSizeList) {
        this.maxHeaderSizeList = maxHeaderSizeList;
        return this;
    }

    public HTTPConfigurationBuilder withMaxInitialLineLength(int maxInitialLineLength) {
        this.maxInitialLineLength = maxInitialLineLength;
        return this;
    }

    public HTTPConfigurationBuilder withMaxHeaderSize(int maxHeaderSize) {
        this.maxHeaderSize = maxHeaderSize;
        return this;
    }

    public HTTPConfigurationBuilder withMaxChunkSize(int maxChunkSize) {
        this.maxChunkSize = maxChunkSize;
        return this;
    }

    public HTTPConfigurationBuilder withCompressionThreshold(int compressionThreshold) {
        this.compressionThreshold = compressionThreshold;
        return this;
    }

    public HTTPConfigurationBuilder withDeflateCompressionLevel(int deflateCompressionLevel) {
        this.deflateCompressionLevel = deflateCompressionLevel;
        return this;
    }

    public HTTPConfigurationBuilder withBrotliCompressionLevel(int brotliCompressionLevel) {
        this.brotliCompressionLevel = brotliCompressionLevel;
        return this;
    }

    public HTTPConfigurationBuilder withEnableHTTP2Push(boolean enableHTTP2Push) {
        this.enableHTTP2Push = enableHTTP2Push;
        return this;
    }

    public HTTPConfiguration build() {
        HTTPConfiguration hTTPConfiguration = new HTTPConfiguration();
        hTTPConfiguration.setMaxContentLength(ObjectUtil.checkPositive(maxContentLength, "Max ContentLength"));
        hTTPConfiguration.setInitialWindowSize(ObjectUtil.checkPositive(initialWindowSize, "Initial Window Size"));
        hTTPConfiguration.setMaxConcurrentStreams(ObjectUtil.checkPositive(maxConcurrentStreams, "Max Concurrent Streams"));
        hTTPConfiguration.setMaxHeaderSizeList(ObjectUtil.checkPositive(maxHeaderSizeList, "Max Header Size List"));
        hTTPConfiguration.setMaxInitialLineLength(ObjectUtil.checkPositive(maxInitialLineLength, "Max Initial Line Length"));
        hTTPConfiguration.setMaxHeaderSize(ObjectUtil.checkPositive(maxHeaderSize, "Max Header Size"));
        hTTPConfiguration.setMaxChunkSize(ObjectUtil.checkPositive(maxChunkSize, "Max Chunk Size"));
        hTTPConfiguration.setCompressionThreshold(ObjectUtil.checkPositive(compressionThreshold, "Max Compression Threshold"));
        hTTPConfiguration.setDeflateCompressionLevel(ObjectUtil.checkPositive(deflateCompressionLevel, "Deflate Compression Level"));
        hTTPConfiguration.setBrotliCompressionLevel(ObjectUtil.checkPositive(brotliCompressionLevel, "Brotli Compression Level"));
        hTTPConfiguration.setEnableHTTP2Push(enableHTTP2Push);
        return hTTPConfiguration;
    }
}
