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
import io.netty.util.internal.ObjectUtil;

/**
 * Configuration for HTTP
 */
public final class HTTPConfiguration extends GenericConfiguration {
    private long maxContentLength;
    private int h2InitialWindowSize;
    private long h2MaxConcurrentStreams;
    private long h2MaxHeaderSizeList;
    private long h2MaxHeaderTableSize;
    private int h2MaxFrameSize;
    private boolean h2enablePush;
    private int maxInitialLineLength;
    private int maxHeaderSize;
    private int maxChunkSize;
    private int compressionThreshold;
    private int deflateCompressionLevel;
    private int brotliCompressionLevel;

    HTTPConfiguration() {
        // Prevent outside initialization
    }

    public long getMaxContentLength() {
        return maxContentLength;
    }

    public void setMaxContentLength(long maxContentLength) {
        this.maxContentLength = ObjectUtil.checkPositive(maxContentLength, "maxContentLength");
    }

    public int getH2InitialWindowSize() {
        return h2InitialWindowSize;
    }

    public void setH2InitialWindowSize(int h2InitialWindowSize) {
        this.h2InitialWindowSize = ObjectUtil.checkPositive(h2InitialWindowSize, "h2InitialWindowSize");
    }

    public long getH2MaxConcurrentStreams() {
        return h2MaxConcurrentStreams;
    }

    public void setH2MaxConcurrentStreams(long h2MaxConcurrentStreams) {
        this.h2MaxConcurrentStreams = ObjectUtil.checkPositive(h2MaxConcurrentStreams, "h2MaxConcurrentStreams");
    }

    public long getH2MaxHeaderSizeList() {
        return h2MaxHeaderSizeList;
    }

    public void setH2MaxHeaderSizeList(long h2MaxHeaderSizeList) {
        this.h2MaxHeaderSizeList = ObjectUtil.checkPositive(h2MaxHeaderSizeList, "h2MaxHeaderSizeList");
    }

    public long getH2MaxHeaderTableSize() {
        return h2MaxHeaderTableSize;
    }

    public void setH2MaxHeaderTableSize(long h2MaxHeaderTableSize) {
        this.h2MaxHeaderTableSize = ObjectUtil.checkPositive(h2MaxHeaderTableSize, "h2MaxHeaderTableSize");
    }

    public int getH2MaxFrameSize() {
        return h2MaxFrameSize;
    }

    public void setH2MaxFrameSize(int h2MaxFrameSize) {
        this.h2MaxFrameSize = ObjectUtil.checkPositive(h2MaxFrameSize, "h2MaxFrameSize");
    }

    public boolean isH2enablePush() {
        return h2enablePush;
    }

    public void setH2enablePush(boolean h2enablePush) {
        this.h2enablePush = h2enablePush;
    }

    public int getMaxInitialLineLength() {
        return maxInitialLineLength;
    }

    public void setMaxInitialLineLength(int maxInitialLineLength) {
        this.maxInitialLineLength = ObjectUtil.checkPositive(maxInitialLineLength, "maxInitialLineLength");
    }

    public int getMaxHeaderSize() {
        return maxHeaderSize;
    }

    public void setMaxHeaderSize(int maxHeaderSize) {
        this.maxHeaderSize = ObjectUtil.checkPositive(maxHeaderSize, "maxHeaderSize");
    }

    public int getMaxChunkSize() {
        return maxChunkSize;
    }

    public void setMaxChunkSize(int maxChunkSize) {
        this.maxChunkSize = ObjectUtil.checkPositive(maxChunkSize, "maxChunkSize");
    }

    public int getCompressionThreshold() {
        return compressionThreshold;
    }

    public void setCompressionThreshold(int compressionThreshold) {
        this.compressionThreshold = ObjectUtil.checkPositiveOrZero(compressionThreshold, "compressionThreshold");
    }

    public int getDeflateCompressionLevel() {
        return deflateCompressionLevel;
    }

    public void setDeflateCompressionLevel(int deflateCompressionLevel) {
        this.deflateCompressionLevel = ObjectUtil.checkInRange(deflateCompressionLevel, 0, 9, "deflateCompressionLevel");
    }

    public int getBrotliCompressionLevel() {
        return brotliCompressionLevel;
    }

    public void setBrotliCompressionLevel(int brotliCompressionLevel) {
        this.brotliCompressionLevel = ObjectUtil.checkInRange(brotliCompressionLevel, 1, 11, "brotliCompressionLevel");
    }
}
