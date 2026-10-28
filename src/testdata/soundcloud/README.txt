Synthetic sine-wave fixtures, generated with ffmpeg. No SoundCloud audio or
credentials are stored here. The binary and playlist fixtures total about 13 KB.

ffmpeg -f lavfi -i sine=frequency=440:sample_rate=22050:duration=1.2 -ac 1 -c:a libmp3lame -b:a 32k tone.mp3
ffmpeg -f lavfi -i sine=frequency=660:sample_rate=44100:duration=1.2 -ac 2 -c:a aac -b:a 32k -f hls -hls_time 0.4 -hls_segment_type fmp4 -hls_segment_filename aac%d.m4s -hls_playlist_type vod aac.m3u8
ffmpeg -f lavfi -i sine=frequency=880:sample_rate=44100:duration=0.4 -ac 1 -c:a aac -b:a 24k -f adts tone.aac

The subsecond HLS target duration emitted by ffmpeg is normalized to 1.
