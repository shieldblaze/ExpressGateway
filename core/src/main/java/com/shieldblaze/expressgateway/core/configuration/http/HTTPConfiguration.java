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

import com.shieldblaze.expressgateway.core.configuration.GenericConfiguration;

/**
 * Configuration for HTTP
 */
public final class HTTPConfiguration extends GenericConfiguration {
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

    HTTPConfiguration() {
        // Prevent outside initialization
    }

    public long getMaxContentLength() {
        return maxContentLength;
    }

    public int getInitialWindowSize() {
        return initialWindowSize;
    }

    public int getMaxConcurrentStreams() {
        return maxConcurrentStreams;
    }

    public long getMaxHeaderSizeList() {
        return maxHeaderSizeList;
    }

    public int getMaxInitialLineLength() {
        return maxInitialLineLength;
    }

    public int getMaxHeaderSize() {
        return maxHeaderSize;
    }

    public int getMaxChunkSize() {
        return maxChunkSize;
    }

    public int getCompressionThreshold() {
        return compressionThreshold;
    }

    public int getDeflateCompressionLevel() {
        return deflateCompressionLevel;
    }

    public int getBrotliCompressionLevel() {
        return brotliCompressionLevel;
    }

    public boolean enableHTTP2Push() {
        return enableHTTP2Push;
    }

    void setMaxContentLength(long maxContentLength) {
        this.maxContentLength = maxContentLength;
    }

    void setInitialWindowSize(int initialWindowSize) {
        this.initialWindowSize = initialWindowSize;
    }

    void setMaxConcurrentStreams(int maxConcurrentStreams) {
        this.maxConcurrentStreams = maxConcurrentStreams;
    }

    void setMaxHeaderSizeList(long maxHeaderSizeList) {
        this.maxHeaderSizeList = maxHeaderSizeList;
    }

    void setMaxInitialLineLength(int maxInitialLineLength) {
        this.maxInitialLineLength = maxInitialLineLength;
    }

    void setMaxHeaderSize(int maxHeaderSize) {
        this.maxHeaderSize = maxHeaderSize;
    }

    void setMaxChunkSize(int maxChunkSize) {
        this.maxChunkSize = maxChunkSize;
    }

    void setCompressionThreshold(int compressionThreshold) {
        this.compressionThreshold = compressionThreshold;
    }

    void setDeflateCompressionLevel(int deflateCompressionLevel) {
        this.deflateCompressionLevel = deflateCompressionLevel;
    }

    void setBrotliCompressionLevel(int brotliCompressionLevel) {
        this.brotliCompressionLevel = brotliCompressionLevel;
    }

    void setEnableHTTP2Push(boolean enableHTTP2Push) {
        this.enableHTTP2Push = enableHTTP2Push;
    }
}
