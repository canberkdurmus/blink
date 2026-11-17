//! Sidecars in the styles of the apps Blink interoperates with (RATE-4, SPEC §11.3). Each mirrors
//! the structure those apps write: namespaces, attribute vs element form, foreign fields.

/// Lightroom Classic: attributes on `rdf:Description`, lots of `crs:` foreign content.
pub fn lightroom(rating: i8, label: Option<&str>) -> String {
    let label = label
        .map(|l| format!("\n   xmp:Label=\"{l}\""))
        .unwrap_or_default();
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000 1.000000, 0000/00/00-00:00:00        ">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:tiff="http://ns.adobe.com/tiff/1.0/"
    xmlns:exif="http://ns.adobe.com/exif/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   tiff:Make="SONY"
   tiff:Model="ILCE-1"
   xmp:ModifyDate="2024-05-01T10:00:00.00+02:00"
   xmp:Rating="{rating}"{label}
   crs:Version="16.0"
   crs:ProcessVersion="11.0"
   crs:WhiteBalance="As Shot">
   <dc:subject>
    <rdf:Bag>
     <rdf:li>beach</rdf:li>
    </rdf:Bag>
   </dc:subject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#
    )
}

/// darktable: attributes, `darktable:` history; colour labels live in `darktable:colorlabels`,
/// so there is no `xmp:Label`.
pub fn darktable(rating: i8) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 4.4.0-Exiv2">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:exif="http://ns.adobe.com/exif/1.0/"
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"
    xmlns:darktable="http://darktable.sf.net/"
   exif:DateTimeOriginal="2024:05:01 10:00:00"
   xmp:Rating="{rating}"
   xmpMM:DerivedFrom="DSC00001.ARW"
   darktable:import_timestamp="63851234567000000"
   darktable:xmp_version="5"
   darktable:raw_params="0"
   darktable:auto_presets_applied="1">
   <darktable:colorlabels>
    <rdf:Seq>
     <rdf:li>0</rdf:li>
    </rdf:Seq>
   </darktable:colorlabels>
   <darktable:history>
    <rdf:Seq/>
   </darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#
    )
}

/// Adobe Bridge: xpacket wrapper with BOM, element form.
pub fn bridge(rating: i8, label: Option<&str>) -> String {
    let label = label
        .map(|l| format!("\n         <xmp:Label>{l}</xmp:Label>"))
        .unwrap_or_default();
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 9.1-c001 79.675d0f7, 2023/06/11-19:21:16        \">
   <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">
      <rdf:Description rdf:about=\"\"
            xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"
            xmlns:photoshop=\"http://ns.adobe.com/photoshop/1.0/\">
         <xmp:Rating>{rating}</xmp:Rating>{label}
         <xmp:MetadataDate>2024-05-01T10:00:00+02:00</xmp:MetadataDate>
         <photoshop:DateCreated>2024-05-01T10:00:00</photoshop:DateCreated>
      </rdf:Description>
   </rdf:RDF>
</x:xmpmeta>
<?xpacket end=\"w\"?>"
    )
}

/// digiKam (Exiv2): attributes, plus Microsoft and digiKam rating/label fields that must be ignored.
pub fn digikam(rating: i8, label: Option<&str>) -> String {
    let label = label
        .map(|l| format!("\n   xmp:Label=\"{l}\""))
        .unwrap_or_default();
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"XMP Core 4.4.0-Exiv2\">
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">
  <rdf:Description rdf:about=\"\"
    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"
    xmlns:MicrosoftPhoto=\"http://ns.microsoft.com/photo/1.0/\"
    xmlns:digiKam=\"http://www.digikam.org/ns/1.0/\"
   xmp:Rating=\"{rating}\"{label}
   MicrosoftPhoto:Rating=\"99\"
   digiKam:ColorLabel=\"1\"
   digiKam:PickLabel=\"3\">
   <digiKam:TagsList>
    <rdf:Seq>
     <rdf:li>Places/Beach</rdf:li>
    </rdf:Seq>
   </digiKam:TagsList>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end=\"w\"?>"
    )
}

/// An old Adobe sidecar that binds the XMP namespace to the `xap:` prefix.
pub fn xap_prefix(rating: i8) -> String {
    format!(
        r#"<x:xapmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xap="http://ns.adobe.com/xap/1.0/" xap:Rating="{rating}"/></rdf:RDF></x:xapmeta>"#
    )
}
