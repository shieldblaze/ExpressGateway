/**
 * <p> How HTTP Compression Works: </p>
 * <p> ------------------------------------------------------ </p>
 * <p> HTTP/2 Inbound: </p>
 * <p> When Inbound receives a new stream, we look up at HTTP/2 Headers and
 * try to find {@code Content-Encoding}. If {@code Content-Encoding} is present then
 * it uses {@code HTTP2ContentDecompressor} to determine if {@code Content-Encoding} type
 * is supported.
 * <ul>
 *     <li>
 *          <p>
 *          If {@code Content-Encoding} type is supported then we'll go ahead and decompress the
 *          HTTP/2 Data Frames of the stream and remove {@code Content-Encoding} header and pass
 *          the data down to {@code InboundHttp2ToHttpObjectAdapter}.
 *          </p>
 *     </li>
 *     <li>
 *          <p>
 *          If {@code Content-Encoding} type is not supported then we'll not do anything and
 *          pass everything down to {@code InboundHttp2ToHttpObjectAdapter}.
 *          </p>
 *     </li>
 * </ul>
 * </p>
 * <p> ------------------------------------------------------ </p>
 * <p> ------------------------------------------------------ </p>
 * <p> HTTP/2 Outbound: </p>
 * <p> When Outbound receives a new stream, we look  at HTTP/2 Headers and
 * try to find {@code Content-Encoding}. If {@code Content-Encoding} is present then
 * it uses {@code HTTP2ContentCompressor} to determine if {@code Content-Encoding} type
 * is supported.
 * <ul>
 *     <li>
 *          <p>
 *          If {@code Content-Encoding} type is supported then we'll go ahead and compress the
 *          HTTP/2 Data Frames of the stream and pass the data down to
 *          {@code InboundHttp2ToHttpObjectAdapter}.
 *          </p>
 *     </li>
 *     <li>
 *          <p>
 *          If {@code Content-Encoding} type is not supported then we'll not do anything and
 *          pass everything down to {@code InboundHttp2ToHttpObjectAdapter}.
 *          </p>
 *     </li>
 * </ul>
 * </p>
 * <p> ------------------------------------------------------ </p>
 * <p> HTTP/1.1 Inbound: </p>
 * <p> When a new HTTP request is received, we look up at HTTP Headers and
 * try to find {@code Content-Encoding}. If {@code Content-Encoding} is present then
 * it uses {@code HTTPContentDecompressor} to determine if {@code Content-Encoding} type
 * is supported.
 * <ul>
 *     <li>
 *          <p>
 *          If {@code Content-Encoding} type is supported then we'll go ahead and decompress the
 *          HTTP Content remove {@code Content-Encoding} header and pass
 *          the data down to {@code UpstreamHandler}.
 *          </p>
 *     </li>
 *     <li>
 *          <p>
 *          If {@code Content-Encoding} type is not supported then we'll not do anything and
 *          pass everything down to {@code UpstreamHandler}.
 *          </p>
 *     </li>
 * </ul>
 * </p>
 * <p> ------------------------------------------------------ </p>
 * <p> ------------------------------------------------------ </p>
 * <p> HTTP/1.1 Outbound: </p>
 * <p> When a new HTTP response is received, we look up at HTTP Headers and
 * try to find {@code Content-Encoding}. If {@code Content-Encoding} is present then
 * it uses {@code HTTPContentCompressor} to determine if {@code Content-Encoding} type
 * is supported.
 * <ul>
 *     <li>
 *          <p>
 *          If {@code Content-Encoding} type is supported then we'll go ahead and decompress the
 *          HTTP Content remove {@code Content-Encoding} header and pass
 *          the data down to {@code UpstreamHandler}.
 *          </p>
 *     </li>
 *     <li>
 *          <p>
 *          If {@code Content-Encoding} type is not supported then we'll not do anything and
 *          pass everything down to {@code UpstreamHandler}.
 *          </p>
 *     </li>
 * </ul>
 * </p>
 * <p> ------------------------------------------------------ </p>
 * <p> ------------------------------------------------------ </p>
 * <p> Note: Currently, only {@code br}, {@code gzip} and {@code deflate} compression
 * and decompression is supported. </p>
 */
package com.shieldblaze.expressgateway.core.server.http.compression;
