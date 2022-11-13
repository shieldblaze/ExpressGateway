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
package com.shieldblaze.expressgateway.configuration.http;

import com.fasterxml.jackson.annotation.JsonIgnore;
import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.configuration.Configuration;

import static io.netty.handler.codec.http.HttpObjectDecoder.DEFAULT_MAX_CHUNK_SIZE;
import static io.netty.handler.codec.http.HttpObjectDecoder.DEFAULT_MAX_HEADER_SIZE;
import static io.netty.handler.codec.http.HttpObjectDecoder.DEFAULT_MAX_INITIAL_LINE_LENGTH;

/**
 * Configuration for HTTP
 */
public final class HttpConfiguration implements Configuration<HttpConfiguration> {

    @JsonProperty
    private int maxInitialLineLength;

    @JsonProperty
    private int maxHeaderSize;

    @JsonProperty
    private int maxChunkSize;

    @JsonProperty
    private int compressionThreshold;

    @JsonProperty
    private int deflateCompressionLevel;

    @JsonProperty
    private int brotliCompressionLevel;

    @JsonIgnore
    private boolean validated;

    public static final HttpConfiguration DEFAULT = new HttpConfiguration();

    static {
        DEFAULT.maxInitialLineLength = DEFAULT_MAX_INITIAL_LINE_LENGTH;
        DEFAULT.maxHeaderSize = DEFAULT_MAX_HEADER_SIZE;
        DEFAULT.maxChunkSize = DEFAULT_MAX_CHUNK_SIZE;
        DEFAULT.compressionThreshold = 1024;
        DEFAULT.deflateCompressionLevel = 6;
        DEFAULT.brotliCompressionLevel = 4;
        DEFAULT.validated = true;
    }

    HttpConfiguration() {
        // Prevent outside initialization
    }

    /**
     * Max Initial Line Length
     */
    public HttpConfiguration setMaxInitialLineLength(int maxInitialLineLength) {
        this.maxInitialLineLength = maxInitialLineLength;
        return this;
    }

    /**
     * Max Initial Line Length
     */
    public int maxInitialLineLength() {
        assertValidated();
        return maxInitialLineLength;
    }

    /**
     * Max Header Size
     */
    public HttpConfiguration setMaxHeaderSize(int maxHeaderSize) {
        this.maxHeaderSize = maxHeaderSize;
        return this;
    }

    /**
     * Max Header Size
     */
    public int maxHeaderSize() {
        assertValidated();
        return maxHeaderSize;
    }

    /**
     * Max Chunk Size
     */
    public HttpConfiguration setMaxChunkSize(int maxChunkSize) {
        this.maxChunkSize = maxChunkSize;
        return this;
    }

    /**
     * Max Chunk Size
     */
    public int maxChunkSize() {
        assertValidated();
        return maxChunkSize;
    }

    /**
     * Compression Threshold
     */
    public HttpConfiguration setCompressionThreshold(int compressionThreshold) {
        this.compressionThreshold = compressionThreshold;
        return this;
    }

    /**
     * Compression Threshold
     */
    public int compressionThreshold() {
        assertValidated();
        return compressionThreshold;
    }

    /**
     * Deflate Compression Level
     */
    public HttpConfiguration setDeflateCompressionLevel(int deflateCompressionLevel) {
        this.deflateCompressionLevel = deflateCompressionLevel;
        return this;
    }

    public int deflateCompressionLevel() {
        assertValidated();
        return deflateCompressionLevel;
    }

    /**
     * Brotli Compression Level
     */
    public HttpConfiguration setBrotliCompressionLevel(int brotliCompressionLevel) {
        this.brotliCompressionLevel = brotliCompressionLevel;
        return this;
    }

    /**
     * Brotli Compression Level
     */
    public int brotliCompressionLevel() {
        assertValidated();
        return brotliCompressionLevel;
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     */
    public HttpConfiguration validate() throws IllegalArgumentException {
        NumberUtil.checkPositive(maxInitialLineLength, "MaxInitialLineLength");
        NumberUtil.checkPositive(maxHeaderSize, "MaxHeaderSize");
        NumberUtil.checkPositive(maxChunkSize, "MaxChunkSize");
        NumberUtil.checkZeroOrPositive(compressionThreshold, "CompressionThreshold");
        NumberUtil.checkInRange(deflateCompressionLevel, 0, 9, "DeflateCompressionLevel");
        NumberUtil.checkInRange(brotliCompressionLevel, 1, 11, "BrotliCompressionLevel");
        validated = true;
        return this;
    }

    @Override
    public boolean validated() {
        return validated;
    }
}
