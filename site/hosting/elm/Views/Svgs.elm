module Views.Svgs exposing (..)

import Html
import Svg exposing (..)
import Svg.Attributes exposing (..)
import Messages exposing (Msg)



joinStr2 : String -> String -> Int -> String
joinStr2 str str2 int =
    if int == 0 then
        str2
    else
        joinStr2 str (str2 ++ str) (int - 1)

joinStr : String -> Int -> String
joinStr str int =
    if int == 0 then
        str
    else
        joinStr2 str str (int - 1)


terminalAnimation : Html.Html Msg
terminalAnimation =
    svg [ id "terminal"
        , baseProfile "full"
        , viewBox "-150 -55 660 512"
        , width "660"
        , version "1.1"
        ]
        [ svg [ id "screen"
              , width "660"
              , fillOpacity "1.0"
              , viewBox "0 0 660 510"
              , preserveAspectRatio "xMidYMin meet"
              ]
              [ defs []
                    [ g [ id "g1" ]
                        [ text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "\xA0" ]
                        ]
                    , g [ id "g2" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "8"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "16"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "912"
                                , x "24"
                                ]
                                [ text (joinStr "\xA0" 114) ]
                        ]
                    , g [ id "g3" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "16"
                                , x "8"
                                ]
                                [ text "\xA0p" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "24"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "904"
                                , x "32"
                                ]
                                [ text (joinStr "\xA0" 113) ]
                        ]
                    , g [ id "g4" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "24"
                                , x "8"
                                ]
                                [ text "\xA0po" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "32"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "896"
                                , x "40"
                                ]
                                [ text (joinStr "\xA0" 112) ]
                        ]
                    , g [ id "g5" ]
                        [ text_
                              [ class "color5"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "0"
                              ]
                              [ text "❯" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "32"
                              , x "8"
                              ]
                              [ text "\xA0poa" ]
                        , text_
                              [ class "background"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "40"
                              ]
                              [ text "\xA0" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "888"
                              , x "48"
                              ]
                              [ text (joinStr "\xA0" 111) ]
                        ]
                    , g [ id "g6" ]
                        [ text_
                              [ class "color5"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "0"
                              ]
                              [ text "❯" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "40"
                              , x "8"
                              ]
                              [ text "\xA0poac" ]
                        , text_
                              [ class "background"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "48"
                              ]
                              [ text "\xA0" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "880"
                              , x "56"
                              ]
                              [ text (joinStr "\xA0" 110) ]
                        ]
                    , g [ id "g7" ]
                        [ text_
                              [ class "color5"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "0"
                              ]
                              [ text "❯" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "48"
                              , x "8"
                              ]
                              [ text "\xA0poac\xA0" ]
                        , text_
                              [ class "background"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "56"
                              ]
                              [ text "\xA0" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "872"
                              , x "64"
                              ]
                              [ text (joinStr "\xA0" 109) ]
                        ]
                    , g [ id "g8" ]
                        [ text_
                              [ class "color5"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "0"
                              ]
                              [ text "❯" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "56"
                              , x "8"
                              ]
                              [ text "\xA0poac\xA0n" ]
                        , text_
                              [ class "background"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "64"
                              ]
                              [ text "\xA0" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "864"
                              , x "72"
                              ]
                              [ text (joinStr "\xA0" 108) ]
                        ]
                    , g [ id "g9" ]
                        [ text_
                              [ class "color5"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "0"
                              ]
                              [ text "❯" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "64"
                              , x "8"
                              ]
                              [ text "\xA0poac\xA0ne" ]
                        , text_
                              [ class "background"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "72"
                              ]
                              [ text "\xA0" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "856"
                              , x "80"
                              ]
                              [ text (joinStr "\xA0" 107) ]
                        ]
                    , g [ id "g10" ]
                        [ text_
                              [ class "color5"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "0"
                              ]
                              [ text "❯" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "72"
                              , x "8"
                              ]
                              [ text "\xA0poac\xA0new" ]
                        , text_
                              [ class "background"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "80"
                              ]
                              [ text "\xA0" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "848"
                              , x "88"
                              ]
                              [ text (joinStr "\xA0" 106) ]
                        ]
                    , g [ id "g11" ]
                        [ text_
                              [ class "color5"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "0"
                              ]
                              [ text "❯" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "80"
                              , x "8"
                              ]
                              [ text "\xA0poac\xA0new\xA0" ]
                        , text_
                              [ class "background"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "88"
                              ]
                              [ text "\xA0" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "840"
                              , x "96"
                              ]
                              [ text (joinStr "\xA0" 105) ]
                        ]
                    , g [ id "g12" ]
                        [ text_
                              [ class "color5"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "0"
                              ]
                              [ text "❯" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "88"
                              , x "8"
                              ]
                              [ text "\xA0poac\xA0new\xA0m" ]
                        , text_
                              [ class "background"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "96"
                              ]
                              [ text "\xA0" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "832"
                              , x "104"
                              ]
                              [ text (joinStr "\xA0" 104) ]
                        ]
                    , g [ id "g13" ]
                        [ text_
                              [ class "color5"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "0"
                              ]
                              [ text "❯" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "96"
                              , x "8"
                              ]
                              [ text "\xA0poac\xA0new\xA0my" ]
                        , text_
                              [ class "background"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "104"
                              ]
                              [ text "\xA0" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "824"
                              , x "112"
                              ]
                              [ text (joinStr "\xA0" 103) ]
                        ]
                    , g [ id "g14" ]
                        [ text_
                              [ class "color5"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "0"
                              ]
                              [ text "❯" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "104"
                              , x "8"
                              ]
                              [ text "\xA0poac\xA0new\xA0my_" ]
                        , text_
                              [ class "background"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "8"
                              , x "112"
                              ]
                              [ text "\xA0" ]
                        , text_
                              [ class "foreground"
                              , lengthAdjust "spacingAndGlyphs"
                              , textLength "816"
                              , x "120"
                              ]
                              [ text (joinStr "\xA0" 102) ]
                        ]
                    , g [ id "g15" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "112"
                                , x "8"
                                ]
                                [ text "\xA0poac\xA0new\xA0my_p" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "120"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "808"
                                , x "128"
                                ]
                                [ text (joinStr "\xA0" 101) ]
                        ]
                    , g [ id "g16" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "120"
                                , x "8"
                                ]
                                [ text "\xA0poac\xA0new\xA0my_pr" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "128"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "800"
                                , x "136"
                                ]
                                [ text (joinStr "\xA0" 100) ]
                        ]
                    , g [ id "g17" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "128"
                                , x "8"
                                ]
                                [ text "\xA0poac\xA0new\xA0my_pro" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "136"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "792"
                                , x "144"
                                ]
                                [ text (joinStr "\xA0" 99) ]
                        ]
                    , g [ id "g18" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "136"
                                , x "8"
                                ]
                                [ text "\xA0poac\xA0new\xA0my_proj" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "144"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "784"
                                , x "152"
                                ]
                                [ text (joinStr "\xA0" 98) ]
                        ]
                    , g [ id "g20" ]
                        [ text_ [ class "color4"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "32"
                                , x "0"
                                ]
                                [ text "/tmp" ]
                        ]
                    , g [ id "g19" ]
                        [ text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "936"
                                , x "0"
                                ]
                                [ text (joinStr "\xA0" 117) ]
                        ]
                    , g [ id "g21" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "16"
                                , x "8"
                                ]
                                [ text " c" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "24"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "904"
                                , x "32"
                                ]
                                [ text (joinStr "\xA0" 113) ]
                        ]
                    , g [ id "g22" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "24"
                                , x "8"
                                ]
                                [ text " cd" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "32"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "896"
                                , x "40"
                                ]
                                [ text (joinStr "\xA0" 112) ]
                        ]
                    , g [ id "g23" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "32"
                                , x "8"
                                ]
                                [ text " cd\xA0" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "40"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "888"
                                , x "48"
                                ]
                                [ text (joinStr "\xA0" 111) ]
                        ]
                    , g [ id "g24" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "40"
                                , x "8"
                                ]
                                [ text " cd\xA0m" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "48"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "880"
                                , x "56"
                                ]
                                [ text (joinStr "\xA0" 110) ]
                        ]
                    , g [ id "g25" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "48"
                                , x "8"
                                ]
                                [ text " cd\xA0my" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "56"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "872"
                                , x "64"
                                ]
                                [ text (joinStr "\xA0" 109) ]
                        ]
                    , g [ id "g26" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "88"
                                , x "8"
                                ]
                                [ text " cd\xA0my_proj" ]
                        , text_ [ class "foreground"
                                , fontWeight "bold"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "96"
                                ]
                                [ text "/" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "104"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "824"
                                , x "112"
                                ]
                                [ text (joinStr "\xA0" 103) ]
                        ]
                    , g [ id "g27" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "88"
                                , x "8"
                                ]
                                [ text " cd\xA0my_proj" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "96"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "832"
                                , x "104"
                                ]
                                [ text (joinStr "\xA0" 104) ]
                        ]
                    , g [ id "g28" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "16"
                                , x "8"
                                ]
                                [ text "\xA0t" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "24"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "904"
                                , x "32"
                                ]
                                [ text (joinStr "\xA0" 113) ]
                        ]
                    , g [ id "g29" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "24"
                                , x "8"
                                ]
                                [ text "\xA0tr" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "32"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "896"
                                , x "40"
                                ]
                                [ text (joinStr "\xA0" 112) ]
                        ]
                    , g [ id "g30" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "32"
                                , x "8"
                                ]
                                [ text "\xA0tre" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "40"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "888"
                                , x "48"
                                ]
                                [ text (joinStr "\xA0" 111) ]
                        ]
                    , g [ id "g31" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "40"
                                , x "8"
                                ]
                                [ text "\xA0tree" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "48"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "880"
                                , x "56"
                                ]
                                [ text (joinStr "\xA0" 110) ]
                        ]
                    , g [ id "g32" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "48"
                                , x "8"
                                ]
                                [ text "\xA0tree\xA0" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "56"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "872"
                                , x "64"
                                ]
                                [ text (joinStr "\xA0" 109) ]
                        ]
                    , g [ id "g33" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "56"
                                , x "8"
                                ]
                                [ text "\xA0tree\xA0." ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "64"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "864"
                                , x "72"
                                ]
                                [ text (joinStr "\xA0" 108) ]
                        ]
                    , g [ id "g34" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "64"
                                , x "8"
                                ]
                                [ text "\xA0tree\xA0.\xA0" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "72"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "856"
                                , x "80"
                                ]
                                [ text (joinStr "\xA0" 107) ]
                        ]
                    , g [ id "g35" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "72"
                                , x "8"
                                ]
                                [ text "\xA0tree\xA0.\xA0-" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "80"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "848"
                                , x "88"
                                ]
                                [ text (joinStr "\xA0" 106) ]
                        ]
                    , g [ id "g36" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "80"
                                , x "8"
                                ]
                                [ text "\xA0tree\xA0.\xA0-a" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "88"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "840"
                                , x "96"
                                ]
                                [ text (joinStr "\xA0" 105) ]
                        ]
                    , g [ id "g37" ]
                        [ text_ [ class "color4"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "96"
                                , x "0"
                                ]
                                [ text "/tmp/my_proj" ]
                        ]
                    , g [ id "g38" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "56"
                                , x "8"
                                ]
                                [ text "\xA0poac\xA0r" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "64"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "864"
                                , x "72"
                                ]
                                [ text (joinStr "\xA0" 108) ]
                        ]
                    , g [ id "g39" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "64"
                                , x "8"
                                ]
                                [ text "\xA0poac\xA0ru" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "72"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "856"
                                , x "80"
                                ]
                                [ text (joinStr "\xA0" 107) ]
                        ]
                    , g [ id "g40" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "72"
                                , x "8"
                                ]
                                [ text "\xA0poac\xA0run" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "80"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "848"
                                , x "88"
                                ]
                                [ text (joinStr "\xA0" 106) ]
                        ]
                    , g [ id "g41" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "928"
                                , x "8"
                                ]
                                [ text <| "\xA0poac\xA0new\xA0my_proj" ++ (joinStr "\xA0" 99) ]
                        ]
                    , g [ id "g43" ]
                        [ text_ [ class "foreground"
                                , fontWeight "bold"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "256"
                                , x "0"
                                ]
                                [ text "Go\xA0into\xA0your\xA0project\xA0 by\xA0running:" ]
                        ]
                    , g [ id "g44" ]
                        [ text_ [ class "foreground"
                                , fontWeight "bold"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "128"
                                , x "0"
                                ]
                                [ text <| (joinStr "\xA0" 5) ++ "cd my_proj" ]
                        ]
                    , g [ id "g42" ]
                        [ text_ [ class "foreground"
                                , fontWeight "bold"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "384"
                                , x "0"
                                ]
                                [ text "Your\xA0\"my_proj\"\xA0project\xA0was\xA0 created\xA0successfully." ]
                        ]
                    , g [ id "g45" ]
                        [ text_ [ class "foreground"
                                , fontWeight "bold"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "192"
                                , x "0"
                                ]
                                [ text "Start\xA0your\xA0project\xA0with:" ]
                        ]
                    , g [ id "g46" ]
                        [ text_ [ class "foreground"
                                , fontWeight "bold"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "112"
                                , x "0"
                                ]
                                [ text <| (joinStr "\xA0" 5) ++ "poac run" ]
                        ]
                    , g [ id "g47" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "928"
                                , x "8"
                                ]
                                [ text <| " cd\xA0my_proj" ++ (joinStr "\xA0" 105) ]
                        ]
                    , g [ id "g48" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "928"
                                , x "8"
                                ]
                                [ text <| "\xA0tree\xA0.\xA0-a" ++ (joinStr "\xA0" 106) ]
                        ]
                    , g [ id "g52" ]
                        [ text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "96"
                                , x "0"
                                ]
                                [ text "├── main.cpp" ]
                        ]
                    , g [ id "g54" ]
                        [ text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "176"
                                , x "0"
                                ]
                                [ text "0 directories, 4 files" ]
                        ]
                    , g [ id "g53" ]
                        [ text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "96"
                                , x "0"
                                ]
                                [ text "└── poac.yml" ]
                        ]
                    , g [ id "g50" ]
                        [ text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "112"
                                , x "0"
                                ]
                                [ text "├── .gitignore" ]
                        ]
                    , g [ id "g49" ]
                        [ text_ [ class "color4"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "." ]
                        ]
                    , g [ id "g51" ]
                        [ text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "104"
                                , x "0"
                                ]
                                [ text "├── README.md" ]
                        ]
                    , g [ id "g55" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "928"
                                , x "8"
                                ]
                                [ text <| "\xA0poac run" ++ (joinStr "\xA0" 107) ]
                        ]
                    , g [ id "g56" ]
                        [ text_ [ class "color2"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "80"
                                , x "0"
                                ]
                                [ text "Compiled:\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "240"
                                , x "80"
                                ]
                                [ text "Output to `_build/bin/my_proj`" ]
                        ]
                    , g [ id "g57" ]
                        [ text_ [ class "color2"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "72"
                                , x "0"
                                ]
                                [ text "Running:\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "160"
                                , x "72"
                                ]
                                [ text "`_build/bin/my_proj`" ]
                        ]
                    , g [ id "g58" ]
                        [ text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "104"
                                , x "0"
                                ]
                                [ text "Hello, world!" ]
                        ]
                    , g [ id "g59" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯"
                                ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "552"
                                , x "8"
                                ]
                                [ text " echo \"deps:\n  boost/bind: \\\">=1.64.0 and <1.68.0\\\"\" >> poac.yml" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "560"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "544"
                                , x "392"
                                ]
                                [ text (joinStr "\xA0" 68) ]
                        ]
                    , g [ id "g60" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "928"
                                , x "8"
                                ]
                                [ text <| " echo \"deps:\n  boost/bind: \\\">=1.64.0 and <1.68.0\\\"\" >> poac.yml" ++ (joinStr "\xA0" 51) ] -- 47
                        ]
                    , g [ id "g61" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "56"
                                , x "8"
                                ]
                                [ text "\xA0poac\xA0i" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "64"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "864"
                                , x "72"
                                ]
                                [ text (joinStr "\xA0" 108) ]
                        ]
                    , g [ id "g62" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "64"
                                , x "8"
                                ]
                                [ text "\xA0poac\xA0in" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "72"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "856"
                                , x "80"
                                ]
                                [ text (joinStr "\xA0" 107) ]
                        ]
                    , g [ id "g63" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "72"
                                , x "8"
                                ]
                                [ text "\xA0poac\xA0ins" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "80"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "848"
                                , x "88"
                                ]
                                [ text (joinStr "\xA0" 106) ]
                        ]
                    , g [ id "g64" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "80"
                                , x "8"
                                ]
                                [ text "\xA0poac\xA0inst" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "88"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "840"
                                , x "96"
                                ]
                                [ text (joinStr "\xA0" 105) ]
                        ]
                    , g [ id "g65" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "88"
                                , x "8"
                                ]
                                [ text "\xA0poac\xA0insta" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "96"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "832"
                                , x "104"
                                ]
                                [ text (joinStr "\xA0" 104) ]
                        ]
                    , g [ id "g66" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "96"
                                , x "8"
                                ]
                                [ text "\xA0poac\xA0instal" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "104"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "824"
                                , x "112"
                                ]
                                [ text (joinStr "\xA0" 103) ]
                        ]
                    , g [ id "g67" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯"
                                ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "104"
                                , x "8"
                                ]
                                [ text "\xA0poac\xA0install" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "112"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "816"
                                , x "120"
                                ]
                                [ text (joinStr "\xA0" 102) ]
                        ]
                    , g [ id "g68" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "928"
                                , x "8"
                                ]
                                [ text <| "\xA0poac\xA0install" ++ (joinStr "\xA0" 103) ]
                        ]
                    , g [ id "g69" ]
                        [ text_ [ class "color2"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "32"
                                , x "0"
                                ]
                                [ text "==>\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "168"
                                , x "32"
                                ]
                                [ text "Resolving\xA0packages..." ]
                        ]
                    , g [ id "g70" ]
                        [ text_ [ class "color2"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "32"
                                , x "0"
                                ]
                                [ text "==>\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "200"
                                , x "32"
                                ]
                                [ text "Resolving dependencies..." ]
                        ]
                    , g [ id "g71" ]
                        [ text_ [ class "color2"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "32"
                                , x "0"
                                ]
                                [ text "==>\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "88"
                                , x "32"
                                ]
                                [ text "Fetching..." ]
                        ]
                    , g [ id "g72" ]
                        [ text_ [ class "color2"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "40"
                                , x "0"
                                ]
                                [ text "\xA0\xA0●\xA0\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "240"
                                , x "40"
                                ]
                                [ text "boost/bind 1.66.0 (from: poac)" ]
                        ]
                    , g [ id "g73" ]
                        [ text_ [ class "color2"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "40"
                                , x "0"
                                ]
                                [ text "\xA0\xA0●\xA0\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "256"
                                , x "40"
                                ]
                                [ text "boost/assert 1.66.0 (from: poac)" ]
                        ]
                    , g [ id "g74" ]
                        [ text_ [ class "color2"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "40"
                                , x "0"
                                ]
                                [ text "\xA0\xA0●\xA0\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "256"
                                , x "40"
                                ]
                                [ text "boost/config 1.66.0 (from: poac)" ]
                        ]
                    , g [ id "g75" ]
                        [ text_ [ class "color2"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "40"
                                , x "0"
                                ]
                                [ text "\xA0\xA0●\xA0\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "240"
                                , x "40"
                                ]
                                [ text "boost/core 1.66.0 (from: poac)" ]
                        ]
                    , g [ id "g76" ]
                        [ text_ [ class "color2"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "32"
                                , x "0"
                                ]
                                [ text "==>\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "40"
                                , x "32"
                                ]
                                [ text "Done." ]
                        ]
                    , g [ id "g77" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "56"
                                , x "8"
                                ]
                                [ text "\xA0tree d" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "64"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "864"
                                , x "72"
                                ]
                                [ text (joinStr "\xA0" 108) ]
                        ]
                    , g [ id "g78" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "64"
                                , x "8"
                                ]
                                [ text "\xA0tree de" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "72"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "856"
                                , x "80"
                                ]
                                [ text (joinStr "\xA0" 107) ]
                        ]
                    , g [ id "g79" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "72"
                                , x "8"
                                ]
                                [ text "\xA0tree dep" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "80"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "848"
                                , x "88"
                                ]
                                [ text (joinStr "\xA0" 106) ]
                        ]
                    , g [ id "g80" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "80"
                                , x "8"
                                ]
                                [ text "\xA0tree deps" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "88"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "840"
                                , x "96"
                                ]
                                [ text (joinStr "\xA0" 105) ]
                        ]
                    , g [ id "g81" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "88"
                                , x "8"
                                ]
                                [ text "\xA0tree deps\xA0" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "96"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "832"
                                , x "104"
                                ]
                                [ text (joinStr "\xA0" 104) ]
                        ]
                    , g [ id "g82" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "96"
                                , x "8"
                                ]
                                [ text "\xA0tree deps\xA0-" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "104"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "824"
                                , x "112"
                                ]
                                [ text (joinStr "\xA0" 103) ]
                        ]
                    , g [ id "g83" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "104"
                                , x "8"
                                ]
                                [ text "\xA0tree deps\xA0-L" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "112"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "816"
                                , x "120"
                                ]
                                [ text (joinStr "\xA0" 102) ]
                        ]
                    , g [ id "g84" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "112"
                                , x "8"
                                ]
                                [ text "\xA0tree deps\xA0-L\xA0" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "120"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "808"
                                , x "128"
                                ]
                                [ text (joinStr "\xA0" 101) ]
                        ]
                    , g [ id "g85" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "120"
                                , x "8"
                                ]
                                [ text "\xA0tree deps\xA0-L 1" ]
                        , text_ [ class "background"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "128"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "800"
                                , x "136"
                                ]
                                [ text (joinStr "\xA0" 100) ]
                        ]
                    , g [ id "g86" ]
                        [ text_ [ class "color4"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "96"
                                , x "0"
                                ]
                                [ text "/tmp/my_proj" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "96"
                                ]
                                [ text "\xA0" ]
                        , text_ [ class "color3"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "16"
                                , x "104"
                                ]
                                [ text "7s" ]
                        ]
                    , g [ id "g87" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x "0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "928"
                                , x "8"
                                ]
                                [ text ("\xA0tree deps\xA0-L 1" ++ (joinStr "\xA0" 101)) ]
                        ]
                    , g [ id "g92" ]
                        [ text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "32"
                                , x "0"
                                ]
                                [ text "└──\xA0" ]
                        , text_ [ class "color4"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "80"
                                , x "32"
                                ]
                                [ text "boost-core" ]
                        ]
                    , g [ id "g93" ]
                        [ text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "176"
                                , x "0"
                                ]
                                [ text "4 directories, 0 files" ]
                        ]
                    , g [ id "g91" ]
                        [ text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "32"
                                , x "0"
                                ]
                                [ text "├──\xA0" ]
                        , text_ [ class "color4"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "96"
                                , x "32"
                                ]
                                [ text "boost-config" ]
                        ]
                    , g [ id "g88" ]
                        [ text_ [ class "color4"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "32"
                                , x "0"
                                ]
                                [ text "deps" ]
                        ]
                    , g [ id "g89" ]
                        [ text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "32"
                                , x "0"
                                ]
                                [ text "├──\xA0" ]
                        , text_ [ class "color4"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "96"
                                , x "32"
                                ]
                                [ text "boost-assert" ]
                        ]
                    , g [ id "g90" ]
                        [ text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "32"
                                , x "0"
                                ]
                                [ text "├──\xA0" ]
                        , text_ [ class "color4"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "80"
                                , x "32"
                                ]
                                [ text "boost-bind" ]
                        ]
                    , g [ id "g94" ]
                        [ text_ [ class "color5"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "8"
                                , x"0"
                                ]
                                [ text "❯" ]
                        , text_ [ class "foreground"
                                , lengthAdjust "spacingAndGlyphs"
                                , textLength "928"
                                , x "8"
                                ]
                                [ text (joinStr "\xA0" 116) ]
                        ]
                    ]
              , rect [ class "background"
                     , height "100%"
                     , width "100%"
                     , x "0"
                     , y "0"
                     ] []
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "0" ] []
                  , use [ y "0", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "0ms; anim_last.end"
                            , dur "3ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "16", y "34" ] []
                  , use [ y "34", xlinkHref "#g2" ] []
                  , animate [ attributeName "display"
                            , begin "3ms; anim_last.end+3ms"
                            , dur "1412ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "24", y "34" ] []
                  , use [ y "34", xlinkHref "#g3" ] []
                  , animate [ attributeName "display"
                            , begin "1415ms; anim_last.end+1415ms"
                            , dur "50ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "32", y "34" ] []
                  , use [ y "34", xlinkHref "#g4" ] []
                  , animate [ attributeName "display"
                            , begin "1465ms; anim_last.end+1465ms"
                            , dur "69ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "40", y "34" ] []
                  , use [ y "34", xlinkHref "#g5" ] []
                  , animate [ attributeName "display"
                            , begin "1534ms; anim_last.end+1534ms"
                            , dur "79ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "48", y "34" ] []
                  , use [ y "34", xlinkHref "#g6" ] []
                  , animate [ attributeName "display"
                            , begin "1613ms; anim_last.end+1613ms"
                            , dur "306ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "56", y "34" ] []
                  , use [ y "34", xlinkHref "#g7" ] []
                  , animate [ attributeName "display"
                            , begin "1919ms; anim_last.end+1919ms"
                            , dur "228ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "64", y "34" ] []
                  , use [ y "34", xlinkHref "#g8" ] []
                  , animate [ attributeName "display"
                            , begin "2147ms; anim_last.end+2147ms"
                            , dur "81ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "72", y "34" ] []
                  , use [ y "34", xlinkHref "#g9" ] []
                  , animate [ attributeName "display"
                            , begin "2228ms; anim_last.end+2228ms"
                            , dur "73ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "80", y "34" ] []
                  , use [ y "34", xlinkHref "#g10" ] []
                  , animate [ attributeName "display"
                            , begin "2301ms; anim_last.end+2301ms"
                            , dur "249ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "88", y "34" ] []
                  , use [ y "34", xlinkHref "#g11" ] []
                  , animate [ attributeName "display"
                            , begin "2550ms; anim_last.end+2550ms"
                            , dur "137ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "96", y "34" ] []
                  , use [ y "34", xlinkHref "#g12" ] []
                  , animate [ attributeName "display"
                            , begin "2687ms; anim_last.end+2687ms"
                            , dur "255ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "104", y "34" ] []
                  , use [ y "34", xlinkHref "#g13" ] []
                  , animate [ attributeName "display"
                            , begin "2942ms; anim_last.end+2942ms"
                            , dur "275ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "112", y "34" ] []
                  , use [ y "34", xlinkHref "#g14" ] []
                  , animate [ attributeName "display"
                            , begin "3217ms; anim_last.end+3217ms"
                            , dur "302ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "120", y "34" ] []
                  , use [ y "34", xlinkHref "#g15" ] []
                  , animate [ attributeName "display"
                            , begin "3519ms; anim_last.end+3519ms"
                            , dur "249ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "128", y "34" ] []
                  , use [ y "34", xlinkHref "#g16" ] []
                  , animate [ attributeName "display"
                            , begin "3768ms; anim_last.end+3768ms"
                            , dur "103ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "136", y "34" ] []
                  , use [ y "34", xlinkHref "#g17" ] []
                  , animate [ attributeName "display"
                            , begin "3871ms; anim_last.end+3871ms"
                            , dur "269ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "144", y "34" ] []
                  , use [ y "34", xlinkHref "#g18" ] []
                  , animate [ attributeName "display"
                            , begin "4140ms; anim_last.end+4140ms"
                            , dur "477ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g19" ] []
                  , use [ y "17", xlinkHref "#g20" ] []
                  , animate [ attributeName "display"
                            , begin "3ms; anim_last.end+3ms"
                            , dur "4615ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "144", y "34" ] []
                  , use [ y "34", xlinkHref "#g18" ] []
                  , animate [ attributeName "display"
                            , begin "4617ms; anim_last.end+4617ms"
                            , dur "1ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "51" ] []
                  , use [ y "51", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "4618ms; anim_last.end+4618ms"
                            , dur "9ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "51" ] []
                  , use [ y "51", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "4627ms; anim_last.end+4627ms"
                            , dur "3ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "51" ] []
                  , use [ y "51", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "4630ms; anim_last.end+4630ms"
                            , dur "18ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "221" ] []
                  , use [ y "221", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "4648ms; anim_last.end+4648ms"
                            , dur "2ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "221" ] []
                  , use [ y "221", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "4650ms; anim_last.end+4650ms"
                            , dur "3ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "16", y "255" ] []
                  , use [ y "255", xlinkHref "#g2" ] []
                  , animate [ attributeName "display"
                            , begin "4653ms; anim_last.end+4653ms"
                            , dur "1ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "221", xlinkHref "#g19" ] []
                  , use [ y "238", xlinkHref "#g20" ] []
                  , animate [ attributeName "display"
                            , begin "4653ms; anim_last.end+4653ms"
                            , dur "4ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "16", y "255" ] []
                  , use [ y "255", xlinkHref "#g2" ] []
                  , animate [ attributeName "display"
                            , begin "4654ms; anim_last.end+4654ms"
                            , dur "3ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "16", y "255" ] []
                  , use [ y "255", xlinkHref "#g2" ] []
                  , animate [ attributeName "display"
                            , begin "4657ms; anim_last.end+4657ms"
                            , dur "518ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "24", y "255" ] []
                  , use [ y "255", xlinkHref "#g21" ] []
                  , animate [ attributeName "display"
                            , begin "5175ms; anim_last.end+5175ms"
                            , dur "57ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "32", y "255" ] []
                  , use [ y "255", xlinkHref "#g22" ] []
                  , animate [ attributeName "display"
                            , begin "5232ms; anim_last.end+5232ms"
                            , dur "205ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "40", y "255" ] []
                  , use [ y "255", xlinkHref "#g23" ] []
                  , animate [ attributeName "display"
                            , begin "5437ms; anim_last.end+5437ms"
                            , dur "127ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "48", y "255" ] []
                  , use [ y "255", xlinkHref "#g24" ] []
                  , animate [ attributeName "display"
                            , begin "5564ms; anim_last.end+5564ms"
                            , dur "350ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "56", y "255" ] []
                  , use [ y "255", xlinkHref "#g25" ] []
                  , animate [ attributeName "display"
                            , begin "5914ms; anim_last.end+5914ms"
                            , dur "162ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "104", y "255" ] []
                  , use [ y "255", xlinkHref "#g26" ] []
                  , animate [ attributeName "display"
                            , begin "6076ms; anim_last.end+6076ms"
                            , dur "513ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "96", y "255" ] []
                  , use [ y "255", xlinkHref "#g27" ] []
                  , animate [ attributeName "display"
                            , begin "6589ms; anim_last.end+6589ms"
                            , dur "2ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "221", xlinkHref "#g19" ] []
                  , use [ y "238", xlinkHref "#g20" ] []
                  , animate [ attributeName "display"
                            , begin "4657ms; anim_last.end+4657ms"
                            , dur "1935ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "96", y "255" ] []
                  , use [ y "255", xlinkHref "#g27" ] []
                  , animate [ attributeName "display"
                            , begin "6591ms; anim_last.end+6591ms"
                            , dur "1ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "272" ] []
                  , use [ y "272", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "6592ms; anim_last.end+6592ms"
                            , dur "1ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "272" ] []
                  , use [ y "272", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "6593ms; anim_last.end+6593ms"
                            , dur "2ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "16", y "306" ] []
                  , use [ y "306", xlinkHref "#g2" ] []
                  , animate [ attributeName "display"
                            , begin "6595ms; anim_last.end+6595ms"
                            , dur "1113ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "24", y "306" ] []
                  , use [ y "306", xlinkHref "#g28" ] []
                  , animate [ attributeName "display"
                            , begin "7708ms; anim_last.end+7708ms"
                            , dur "253ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "32", y "306" ] []
                  , use [ y "306", xlinkHref "#g29" ] []
                  , animate [ attributeName "display"
                            , begin "7961ms; anim_last.end+7961ms"
                            , dur "365ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "40", y "306" ] []
                  , use [ y "306", xlinkHref "#g30" ] []
                  , animate [ attributeName "display"
                            , begin "8326ms; anim_last.end+8326ms"
                            , dur "134ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "48", y "306" ] []
                  , use [ y "306", xlinkHref "#g31" ] []
                  , animate [ attributeName "display"
                            , begin "8460ms; anim_last.end+8460ms"
                            , dur "330ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "56", y "306" ] []
                  , use [ y "306", xlinkHref "#g32" ] []
                  , animate [ attributeName "display"
                            , begin "8790ms; anim_last.end+8790ms"
                            , dur "571ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "64", y "306" ] []
                  , use [ y "306", xlinkHref "#g33" ] []
                  , animate [ attributeName "display"
                            , begin "9361ms; anim_last.end+9361ms"
                            , dur "379ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "72", y "306" ] []
                  , use [ y "306", xlinkHref "#g34" ] []
                  , animate [ attributeName "display"
                            , begin "9740ms; anim_last.end+9740ms"
                            , dur "225ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "80", y "306" ] []
                  , use [ y "306", xlinkHref "#g35" ] []
                  , animate [ attributeName "display"
                            , begin "9965ms; anim_last.end+9965ms"
                            , dur "259ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "88", y "306" ] []
                  , use [ y "306", xlinkHref "#g36" ] []
                  , animate [ attributeName "display"
                            , begin "10224ms; anim_last.end+10224ms"
                            , dur "212ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "272", xlinkHref "#g19" ] []
                  , use [ y "289", xlinkHref "#g37" ] []
                  , animate [ attributeName "display"
                            , begin "6595ms; anim_last.end+6595ms"
                            , dur "3842ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "88", y "306" ] []
                  , use [ y "306", xlinkHref "#g36" ] []
                  , animate [ attributeName "display"
                            , begin "10436ms; anim_last.end+10436ms"
                            , dur "1ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "323" ] []
                  , use [ y "323", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "10437ms; anim_last.end+10437ms"
                            , dur "5ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "442" ] []
                  , use [ y "442", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "10442ms; anim_last.end+10442ms"
                            , dur "2ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "442" ] []
                  , use [ y "442", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "10444ms; anim_last.end+10444ms"
                            , dur "1ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "16", y "476" ] []
                  , use [ y "476", xlinkHref "#g2" ] []
                  , animate [ attributeName "display"
                            , begin "10445ms; anim_last.end+10445ms"
                            , dur "1611ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "24", y "476" ] []
                  , use [ y "476", xlinkHref "#g3" ] []
                  , animate [ attributeName "display"
                            , begin "12056ms; anim_last.end+12056ms"
                            , dur "49ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "32", y "476" ] []
                  , use [ y "476", xlinkHref "#g4" ] []
                  , animate [ attributeName "display"
                            , begin "12105ms; anim_last.end+12105ms"
                            , dur "105ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "40", y "476" ] []
                  , use [ y "476", xlinkHref "#g5" ] []
                  , animate [ attributeName "display"
                            , begin "12210ms; anim_last.end+12210ms"
                            , dur "74ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "48", y "476" ] []
                  , use [ y "476", xlinkHref "#g6" ] []
                  , animate [ attributeName "display"
                            , begin "12284ms; anim_last.end+12284ms"
                            , dur "301ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "56", y "476" ] []
                  , use [ y "476", xlinkHref "#g7" ] []
                  , animate [ attributeName "display"
                            , begin "12585ms; anim_last.end+12585ms"
                            , dur "208ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "64", y "476" ] []
                  , use [ y "476", xlinkHref "#g38" ] []
                  , animate [ attributeName "display"
                            , begin "12793ms; anim_last.end+12793ms"
                            , dur "129ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "72", y "476" ] []
                  , use [ y "476", xlinkHref "#g39" ] []
                  , animate [ attributeName "display"
                            , begin "12922ms; anim_last.end+12922ms"
                            , dur "105ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "80", y "476" ] []
                  , use [ y "476", xlinkHref "#g40" ] []
                  , animate [ attributeName "display"
                            , begin "13027ms; anim_last.end+13027ms"
                            , dur "821ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "442", xlinkHref "#g19" ] []
                  , use [ y "459", xlinkHref "#g37" ] []
                  , animate [ attributeName "display"
                            , begin "10445ms; anim_last.end+10445ms"
                            , dur "3404ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "80", y "476" ] []
                  , use [ y "476", xlinkHref "#g40" ] []
                  , animate [ attributeName "display"
                            , begin "13848ms; anim_last.end+13848ms"
                            , dur "1ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "13849ms; anim_last.end+13849ms"
                            , dur "1ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g19" ] []
                  , use [ y "17", xlinkHref "#g20" ] []
                  , use [ y "34", xlinkHref "#g41" ] []
                  , animate [ attributeName "display"
                            , begin "4618ms; anim_last.end+4618ms"
                            , dur "9972ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "68", xlinkHref "#g42" ] []
                  , use [ y "119", xlinkHref "#g43" ] []
                  , use [ y "136", xlinkHref "#g44" ] []
                  , use [ y "170", xlinkHref "#g45" ] []
                  , use [ y "187", xlinkHref "#g46" ] []
                  , animate [ attributeName "display"
                            , begin "4648ms; anim_last.end+4648ms"
                            , dur "9942ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "221", xlinkHref "#g19" ] []
                  , use [ y "238", xlinkHref "#g20" ] []
                  , use [ y "255", xlinkHref "#g47" ] []
                  , animate [ attributeName "display"
                            , begin "6592ms; anim_last.end+6592ms"
                            , dur "7998ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "272", xlinkHref "#g19" ] []
                  , use [ y "289", xlinkHref "#g37" ] []
                  , use [ y "306", xlinkHref "#g48" ] []
                  , animate [ attributeName "display"
                            , begin "10437ms; anim_last.end+10437ms"
                            , dur "4153ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "323", xlinkHref "#g49" ] []
                  , use [ y "340", xlinkHref "#g50" ] []
                  , use [ y "357", xlinkHref "#g51" ] []
                  , use [ y "374", xlinkHref "#g52" ] []
                  , use [ y "391", xlinkHref "#g53" ] []
                  , use [ y "425", xlinkHref "#g54" ] []
                  , animate [ attributeName "display"
                            , begin "10442ms; anim_last.end+10442ms"
                            , dur "4148ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "442", xlinkHref "#g19" ] []
                  , use [ y "459", xlinkHref "#g37" ] []
                  , use [ y "476", xlinkHref "#g55" ] []
                  , animate [ attributeName "display"
                            , begin "13849ms; anim_last.end+13849ms"
                            , dur "741ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "13850ms; anim_last.end+13850ms"
                            , dur "740ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g41" ] []
                  , use [ y "34", xlinkHref "#g42" ] []
                  , use [ y "85", xlinkHref "#g43" ] []
                  , use [ y "102", xlinkHref "#g44" ] []
                  , use [ y "136", xlinkHref "#g45" ] []
                  , use [ y "153", xlinkHref "#g46" ] []
                  , use [ y "187", xlinkHref "#g19" ] []
                  , use [ y "204", xlinkHref "#g20" ] []
                  , use [ y "221", xlinkHref "#g47" ] []
                  , use [ y "238", xlinkHref "#g19" ] []
                  , use [ y "255", xlinkHref "#g37" ] []
                  , use [ y "272", xlinkHref "#g48" ] []
                  , use [ y "289", xlinkHref "#g49" ] []
                  , use [ y "306", xlinkHref "#g50" ] []
                  , use [ y "323", xlinkHref "#g51" ] []
                  , use [ y "340", xlinkHref "#g52" ] []
                  , use [ y "357", xlinkHref "#g53" ] []
                  , use [ y "391", xlinkHref "#g54" ] []
                  , use [ y "408", xlinkHref "#g19" ] []
                  , use [ y "425", xlinkHref "#g37" ] []
                  , use [ y "442", xlinkHref "#g55" ] []
                  , use [ y "459", xlinkHref "#g56" ] []
                  , use [ y "476", xlinkHref "#g57" ] []
                  , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "14590ms; anim_last.end+14590ms"
                            , dur "4ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "14594ms; anim_last.end+14594ms"
                            , dur "5ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "17", xlinkHref "#g42" ] []
                  , use [ y "68", xlinkHref "#g43" ] []
                  , use [ y "85", xlinkHref "#g44" ] []
                  , use [ y "119", xlinkHref "#g45" ] []
                  , use [ y "136", xlinkHref "#g46" ] []
                  , use [ y "170", xlinkHref "#g19" ] []
                  , use [ y "187", xlinkHref "#g20" ] []
                  , use [ y "204", xlinkHref "#g47" ] []
                  , use [ y "221", xlinkHref "#g19" ] []
                  , use [ y "238", xlinkHref "#g37" ] []
                  , use [ y "255", xlinkHref "#g48" ] []
                  , use [ y "272", xlinkHref "#g49" ] []
                  , use [ y "289", xlinkHref "#g50" ] []
                  , use [ y "306", xlinkHref "#g51" ] []
                  , use [ y "323", xlinkHref "#g52" ] []
                  , use [ y "340", xlinkHref "#g53" ] []
                  , use [ y "374", xlinkHref "#g54" ] []
                  , use [ y "391", xlinkHref "#g19" ] []
                  , use [ y "408", xlinkHref "#g37" ] []
                  , use [ y "425", xlinkHref "#g55" ] []
                  , use [ y "442", xlinkHref "#g56" ] []
                  , use [ y "459", xlinkHref "#g57" ] []
                  , use [ y "476", xlinkHref "#g58" ] []
                  , animate [ attributeName "display"
                            , begin "14594ms; anim_last.end+14594ms"
                            , dur "10ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "14599ms; anim_last.end+14599ms"
                            , dur "5ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "16", y "493" ] []
                  , use [ y "493", xlinkHref "#g2" ] []
                  , animate [ attributeName "display"
                            , begin "14604ms; anim_last.end+14604ms"
                            , dur "4511ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "384", y "493" ] []
                  , use [ y "493", xlinkHref "#g59" ] []
                  , animate [ attributeName "display"
                            , begin "19115ms; anim_last.end+19115ms"
                            , dur "298ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "384", y "493" ] []
                  , use [ y "493", xlinkHref "#g59" ] []
                  , animate [ attributeName "display"
                            , begin "19413ms; anim_last.end+19413ms"
                            , dur "2ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "34", xlinkHref "#g43" ] []
                  , use [ y "51", xlinkHref "#g44" ] []
                  , use [ y "85", xlinkHref "#g45" ] []
                  , use [ y "102", xlinkHref "#g46" ] []
                  , use [ y "136", xlinkHref "#g19" ] []
                  , use [ y "153", xlinkHref "#g20" ] []
                  , use [ y "170", xlinkHref "#g47" ] []
                  , use [ y "187", xlinkHref "#g19" ] []
                  , use [ y "204", xlinkHref "#g37" ] []
                  , use [ y "221", xlinkHref "#g48" ] []
                  , use [ y "238", xlinkHref "#g49" ] []
                  , use [ y "255", xlinkHref "#g50" ] []
                  , use [ y "272", xlinkHref "#g51" ] []
                  , use [ y "289", xlinkHref "#g52" ] []
                  , use [ y "306", xlinkHref "#g53" ] []
                  , use [ y "340", xlinkHref "#g54" ] []
                  , use [ y "357", xlinkHref "#g19" ] []
                  , use [ y "374", xlinkHref "#g37" ] []
                  , use [ y "391", xlinkHref "#g55" ] []
                  , use [ y "408", xlinkHref "#g56" ] []
                  , use [ y "425", xlinkHref "#g57" ] []
                  , use [ y "442", xlinkHref "#g58" ] []
                  , use [ y "459", xlinkHref "#g19" ] []
                  , use [ y "476", xlinkHref "#g37" ] []
                  , animate [ attributeName "display"
                            , begin "14604ms; anim_last.end+14604ms"
                            , dur "4813ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "384", y "493" ] []
                  , use [ y "493", xlinkHref "#g59" ] []
                  , animate [ attributeName "display"
                            , begin "19415ms; anim_last.end+19415ms"
                            , dur "2ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "17", xlinkHref "#g43" ] []
                  , use [ y "34", xlinkHref "#g44" ] []
                  , use [ y "68", xlinkHref "#g45" ] []
                  , use [ y "85", xlinkHref "#g46" ] []
                  , use [ y "119", xlinkHref "#g19" ] []
                  , use [ y "136", xlinkHref "#g20" ] []
                  , use [ y "153", xlinkHref "#g47" ] []
                  , use [ y "170", xlinkHref "#g19" ] []
                  , use [ y "187", xlinkHref "#g37" ] []
                  , use [ y "204", xlinkHref "#g48" ] []
                  , use [ y "221", xlinkHref "#g49" ] []
                  , use [ y "238", xlinkHref "#g50" ] []
                  , use [ y "255", xlinkHref "#g51" ] []
                  , use [ y "272", xlinkHref "#g52" ] []
                  , use [ y "289", xlinkHref "#g53" ] []
                  , use [ y "323", xlinkHref "#g54" ] []
                  , use [ y "340", xlinkHref "#g19" ] []
                  , use [ y "357", xlinkHref "#g37" ] []
                  , use [ y "374", xlinkHref "#g55" ] []
                  , use [ y "391", xlinkHref "#g56" ] []
                  , use [ y "408", xlinkHref "#g57" ] []
                  , use [ y "425", xlinkHref "#g58" ] []
                  , use [ y "442", xlinkHref "#g19" ] []
                  , use [ y "459", xlinkHref "#g37" ] []
                  , use [ y "476", xlinkHref "#g60" ] []
                  , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "19417ms; anim_last.end+19417ms"
                            , dur "1ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "16", y "493" ] []
                  , use [ y "493", xlinkHref "#g2" ] []
                  , animate [ attributeName "display"
                            , begin "19418ms; anim_last.end+19418ms"
                            , dur "10ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "459", xlinkHref "#g19" ] []
                  , use [ y "476", xlinkHref "#g37" ] []
                  , animate [ attributeName "display"
                            , begin "19418ms; anim_last.end+19418ms"
                            , dur "17ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "16", y "493" ] []
                  , use [ y "493", xlinkHref "#g2" ] []
                  , animate [ attributeName "display"
                            , begin "19428ms; anim_last.end+19428ms"
                            , dur "7ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "16", y "493" ] []
                  , use [ y "493", xlinkHref "#g2" ] []
                  , animate [ attributeName "display"
                            , begin "19435ms; anim_last.end+19435ms"
                            , dur "2250ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "24", y "493" ] []
                  , use [ y "493", xlinkHref "#g3" ] []
                  , animate [ attributeName "display"
                            , begin "21685ms; anim_last.end+21685ms"
                            , dur "67ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "32", y "493" ] []
                  , use [ y "493", xlinkHref "#g4" ] []
                  , animate [ attributeName "display"
                            , begin "21752ms; anim_last.end+21752ms"
                            , dur "41ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "40", y "493" ] []
                  , use [ y "493", xlinkHref "#g5" ] []
                  , animate [ attributeName "display"
                            , begin "21793ms; anim_last.end+21793ms"
                            , dur "42ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "48", y "493" ] []
                  , use [ y "493", xlinkHref "#g6" ] []
                  , animate [ attributeName "display"
                            , begin "21835ms; anim_last.end+21835ms"
                            , dur "181ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "56", y "493" ] []
                  , use [ y "493", xlinkHref "#g7" ] []
                  , animate [ attributeName "display"
                            , begin "22016ms; anim_last.end+22016ms"
                            , dur "133ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "64", y "493" ] []
                  , use [ y "493", xlinkHref "#g61" ] []
                  , animate [ attributeName "display"
                            , begin "22149ms; anim_last.end+22149ms"
                            , dur "79ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "72", y "493" ] []
                  , use [ y "493", xlinkHref "#g62" ] []
                  , animate [ attributeName "display"
                            , begin "22228ms; anim_last.end+22228ms"
                            , dur "96ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "80", y "493" ] []
                  , use [ y "493", xlinkHref "#g63" ] []
                  , animate [ attributeName "display"
                            , begin "22324ms; anim_last.end+22324ms"
                            , dur "148ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "88", y "493" ] []
                  , use [ y "493", xlinkHref "#g64" ] []
                  , animate [ attributeName "display"
                            , begin "22472ms; anim_last.end+22472ms"
                            , dur "127ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "96", y "493" ] []
                  , use [ y "493", xlinkHref "#g65" ] []
                  , animate [ attributeName "display"
                            , begin "22599ms; anim_last.end+22599ms"
                            , dur "160ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "104", y "493" ] []
                  , use [ y "493", xlinkHref "#g66" ] []
                  , animate [ attributeName "display"
                            , begin "22759ms; anim_last.end+22759ms"
                            , dur "102ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "112", y "493" ] []
                  , use [ y "493", xlinkHref "#g67" ] []
                  , animate [ attributeName "display"
                            , begin "22861ms; anim_last.end+22861ms"
                            , dur "293ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g44" ] []
                  , use [ y "34", xlinkHref "#g45" ] []
                  , use [ y "51", xlinkHref "#g46" ] []
                  , use [ y "85", xlinkHref "#g19" ] []
                  , use [ y "102", xlinkHref "#g20" ] []
                  , use [ y "119", xlinkHref "#g47" ] []
                  , use [ y "136", xlinkHref "#g19" ] []
                  , use [ y "153", xlinkHref "#g37" ] []
                  , use [ y "170", xlinkHref "#g48" ] []
                  , use [ y "187", xlinkHref "#g49" ] []
                  , use [ y "204", xlinkHref "#g50" ] []
                  , use [ y "221", xlinkHref "#g51" ] []
                  , use [ y "238", xlinkHref "#g52" ] []
                  , use [ y "255", xlinkHref "#g53" ] []
                  , use [ y "289", xlinkHref "#g54" ] []
                  , use [ y "306", xlinkHref "#g19" ] []
                  , use [ y "323", xlinkHref "#g37" ] []
                  , use [ y "340", xlinkHref "#g55" ] []
                  , use [ y "357", xlinkHref "#g56" ] []
                  , use [ y "374", xlinkHref "#g57" ] []
                  , use [ y "391", xlinkHref "#g58" ] []
                  , use [ y "408", xlinkHref "#g19" ] []
                  , use [ y "425", xlinkHref "#g37" ] []
                  , use [ y "442", xlinkHref "#g60" ] []
                  , animate [ attributeName "display"
                            , begin "19418ms; anim_last.end+19418ms"
                            , dur "3737ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "459", xlinkHref "#g19" ] []
                  , use [ y "476", xlinkHref "#g37" ] []
                  , animate [ attributeName "display"
                            , begin "19435ms; anim_last.end+19435ms"
                            , dur "3720ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "112", y "493" ] []
                  , use [ y "493", xlinkHref "#g67" ] []
                  , animate [ attributeName "display"
                            , begin "23154ms; anim_last.end+23154ms"
                            , dur "1ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "23155ms; anim_last.end+23155ms"
                            , dur "1ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "17", xlinkHref "#g45" ] []
                  , use [ y "34", xlinkHref "#g46" ] []
                  , use [ y "68", xlinkHref "#g19" ] []
                  , use [ y "85", xlinkHref "#g20" ] []
                  , use [ y "102", xlinkHref "#g47" ] []
                  , use [ y "119", xlinkHref "#g19" ] []
                  , use [ y "136", xlinkHref "#g37" ] []
                  , use [ y "153", xlinkHref "#g48" ] []
                  , use [ y "170", xlinkHref "#g49" ] []
                  , use [ y "187", xlinkHref "#g50" ] []
                  , use [ y "204", xlinkHref "#g51" ] []
                  , use [ y "221", xlinkHref "#g52" ] []
                  , use [ y "238", xlinkHref "#g53" ] []
                  , use [ y "272", xlinkHref "#g54" ] []
                  , use [ y "289", xlinkHref "#g19" ] []
                  , use [ y "306", xlinkHref "#g37" ] []
                  , use [ y "323", xlinkHref "#g55" ] []
                  , use [ y "340", xlinkHref "#g56" ] []
                  , use [ y "357", xlinkHref "#g57" ] []
                  , use [ y "374", xlinkHref "#g58" ] []
                  , use [ y "391", xlinkHref "#g19" ] []
                  , use [ y "408", xlinkHref "#g37" ] []
                  , use [ y "425", xlinkHref "#g60" ] []
                  , use [ y "442", xlinkHref "#g19" ] []
                  , use [ y "459", xlinkHref "#g37" ] []
                  , use [ y "476", xlinkHref "#g68" ] []
                  , animate [ attributeName "display"
                            , begin "23155ms; anim_last.end+23155ms"
                            , dur "18ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "23156ms; anim_last.end+23156ms"
                            , dur "17ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g45" ] []
                  , use [ y "17", xlinkHref "#g46" ] []
                  , use [ y "51", xlinkHref "#g19" ] []
                  , use [ y "68", xlinkHref "#g20" ] []
                  , use [ y "85", xlinkHref "#g47" ] []
                  , use [ y "102", xlinkHref "#g19" ] []
                  , use [ y "119", xlinkHref "#g37" ] []
                  , use [ y "136", xlinkHref "#g48" ] []
                  , use [ y "153", xlinkHref "#g49" ] []
                  , use [ y "170", xlinkHref "#g50" ] []
                  , use [ y "187", xlinkHref "#g51" ] []
                  , use [ y "204", xlinkHref "#g52" ] []
                  , use [ y "221", xlinkHref "#g53" ] []
                  , use [ y "255", xlinkHref "#g54" ] []
                  , use [ y "272", xlinkHref "#g19" ] []
                  , use [ y "289", xlinkHref "#g37" ] []
                  , use [ y "306", xlinkHref "#g55" ] []
                  , use [ y "323", xlinkHref "#g56" ] []
                  , use [ y "340", xlinkHref "#g57" ] []
                  , use [ y "357", xlinkHref "#g58" ] []
                  , use [ y "374", xlinkHref "#g19" ] []
                  , use [ y "391", xlinkHref "#g37" ] []
                  , use [ y "408", xlinkHref "#g60" ] []
                  , use [ y "425", xlinkHref "#g19" ] []
                  , use [ y "442", xlinkHref "#g37" ] []
                  , use [ y "459", xlinkHref "#g68" ] []
                  , use [ y "476", xlinkHref "#g69" ] []
                  , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "23173ms; anim_last.end+23173ms"
                            , dur "528ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g46" ] []
                  , use [ y "34", xlinkHref "#g19" ] []
                  , use [ y "51", xlinkHref "#g20" ] []
                  , use [ y "68", xlinkHref "#g47" ] []
                  , use [ y "85", xlinkHref "#g19" ] []
                  , use [ y "102", xlinkHref "#g37" ] []
                  , use [ y "119", xlinkHref "#g48" ] []
                  , use [ y "136", xlinkHref "#g49" ] []
                  , use [ y "153", xlinkHref "#g50" ] []
                  , use [ y "170", xlinkHref "#g51" ] []
                  , use [ y "187", xlinkHref "#g52" ] []
                  , use [ y "204", xlinkHref "#g53" ] []
                  , use [ y "238", xlinkHref "#g54" ] []
                  , use [ y "255", xlinkHref "#g19" ] []
                  , use [ y "272", xlinkHref "#g37" ] []
                  , use [ y "289", xlinkHref "#g55" ] []
                  , use [ y "306", xlinkHref "#g56" ] []
                  , use [ y "323", xlinkHref "#g57" ] []
                  , use [ y "340", xlinkHref "#g58" ] []
                  , use [ y "357", xlinkHref "#g19" ] []
                  , use [ y "374", xlinkHref "#g37" ] []
                  , use [ y "391", xlinkHref "#g60" ] []
                  , use [ y "408", xlinkHref "#g19" ] []
                  , use [ y "425", xlinkHref "#g37" ] []
                  , use [ y "442", xlinkHref "#g68" ] []
                  , use [ y "459", xlinkHref "#g69" ] []
                  , use [ y "476", xlinkHref "#g70" ] []
                  , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "23701ms; anim_last.end+23701ms"
                            , dur "6049ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g19" ] []
                  , use [ y "17", xlinkHref "#g20" ] []
                  , use [ y "34", xlinkHref "#g47" ] []
                  , use [ y "51", xlinkHref "#g19" ] []
                  , use [ y "68", xlinkHref "#g37" ] []
                  , use [ y "85", xlinkHref "#g48" ] []
                  , use [ y "102", xlinkHref "#g49" ] []
                  , use [ y "119", xlinkHref "#g50" ] []
                  , use [ y "136", xlinkHref "#g51" ] []
                  , use [ y "153", xlinkHref "#g52" ] []
                  , use [ y "170", xlinkHref "#g53" ] []
                  , use [ y "204", xlinkHref "#g54" ] []
                  , use [ y "221", xlinkHref "#g19" ] []
                  , use [ y "238", xlinkHref "#g37" ] []
                  , use [ y "255", xlinkHref "#g55" ] []
                  , use [ y "272", xlinkHref "#g56" ] []
                  , use [ y "289", xlinkHref "#g57" ] []
                  , use [ y "306", xlinkHref "#g58" ] []
                  , use [ y "323", xlinkHref "#g19" ] []
                  , use [ y "340", xlinkHref "#g37" ] []
                  , use [ y "357", xlinkHref "#g60" ] []
                  , use [ y "374", xlinkHref "#g19" ] []
                  , use [ y "391", xlinkHref "#g37" ] []
                  , use [ y "408", xlinkHref "#g68" ] []
                  , use [ y "425", xlinkHref "#g69" ] []
                  , use [ y "442", xlinkHref "#g70" ] []
                  , use [ y "459", xlinkHref "#g71" ] []
                  , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "29750ms; anim_last.end+29750ms"
                            , dur "428ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g20" ] []
                  , use [ y "17", xlinkHref "#g47" ] []
                  , use [ y "34", xlinkHref "#g19" ] []
                  , use [ y "51", xlinkHref "#g37" ] []
                  , use [ y "68", xlinkHref "#g48" ] []
                  , use [ y "85", xlinkHref "#g49" ] []
                  , use [ y "102", xlinkHref "#g50" ] []
                  , use [ y "119", xlinkHref "#g51" ] []
                  , use [ y "136", xlinkHref "#g52" ] []
                  , use [ y "153", xlinkHref "#g53" ] []
                  , use [ y "187", xlinkHref "#g54" ] []
                  , use [ y "204", xlinkHref "#g19" ] []
                  , use [ y "221", xlinkHref "#g37" ] []
                  , use [ y "238", xlinkHref "#g55" ] []
                  , use [ y "255", xlinkHref "#g56" ] []
                  , use [ y "272", xlinkHref "#g57" ] []
                  , use [ y "289", xlinkHref "#g58" ] []
                  , use [ y "306", xlinkHref "#g19" ] []
                  , use [ y "323", xlinkHref "#g37" ] []
                  , use [ y "340", xlinkHref "#g60" ] []
                  , use [ y "357", xlinkHref "#g19" ] []
                  , use [ y "374", xlinkHref "#g37" ] []
                  , use [ y "391", xlinkHref "#g68" ] []
                  , use [ y "408", xlinkHref "#g69" ] []
                  , use [ y "425", xlinkHref "#g70" ] []
                  , use [ y "442", xlinkHref "#g71" ] []
                  , use [ y "476", xlinkHref "#g72" ] []
                  , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "30178ms; anim_last.end+30178ms"
                            , dur "7ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g47" ] []
                  , use [ y "17", xlinkHref "#g19" ] []
                  , use [ y "34", xlinkHref "#g37" ] []
                  , use [ y "51", xlinkHref "#g48" ] []
                  , use [ y "68", xlinkHref "#g49" ] []
                  , use [ y "85", xlinkHref "#g50" ] []
                  , use [ y "102", xlinkHref "#g51" ] []
                  , use [ y "119", xlinkHref "#g52" ] []
                  , use [ y "136", xlinkHref "#g53" ] []
                  , use [ y "170", xlinkHref "#g54" ] []
                  , use [ y "187", xlinkHref "#g19" ] []
                  , use [ y "204", xlinkHref "#g37" ] []
                  , use [ y "221", xlinkHref "#g55" ] []
                  , use [ y "238", xlinkHref "#g56" ] []
                  , use [ y "255", xlinkHref "#g57" ] []
                  , use [ y "272", xlinkHref "#g58" ] []
                  , use [ y "289", xlinkHref "#g19" ] []
                  , use [ y "306", xlinkHref "#g37" ] []
                  , use [ y "323", xlinkHref "#g60" ] []
                  , use [ y "340", xlinkHref "#g19" ] []
                  , use [ y "357", xlinkHref "#g37" ] []
                  , use [ y "374", xlinkHref "#g68" ] []
                  , use [ y "391", xlinkHref "#g69" ] []
                  , use [ y "408", xlinkHref "#g70" ] []
                  , use [ y "425", xlinkHref "#g71" ] []
                  , use [ y "459", xlinkHref "#g72" ] []
                  , use [ y "476", xlinkHref "#g73" ] []
                  , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "30185ms; anim_last.end+30185ms"
                            , dur "159ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g19" ] []
                  , use [ y "17", xlinkHref "#g37" ] []
                  , use [ y "34", xlinkHref "#g48" ] []
                  , use [ y "51", xlinkHref "#g49" ] []
                  , use [ y "68", xlinkHref "#g50" ] []
                  , use [ y "85", xlinkHref "#g51" ] []
                  , use [ y "102", xlinkHref "#g52" ] []
                  , use [ y "119", xlinkHref "#g53" ] []
                  , use [ y "153", xlinkHref "#g54" ] []
                  , use [ y "170", xlinkHref "#g19" ] []
                  , use [ y "187", xlinkHref "#g37" ] []
                  , use [ y "204", xlinkHref "#g55" ] []
                  , use [ y "221", xlinkHref "#g56" ] []
                  , use [ y "238", xlinkHref "#g57" ] []
                  , use [ y "255", xlinkHref "#g58" ] []
                  , use [ y "272", xlinkHref "#g19" ] []
                  , use [ y "289", xlinkHref "#g37" ] []
                  , use [ y "306", xlinkHref "#g60" ] []
                  , use [ y "323", xlinkHref "#g19" ] []
                  , use [ y "340", xlinkHref "#g37" ] []
                  , use [ y "357", xlinkHref "#g68" ] []
                  , use [ y "374", xlinkHref "#g69" ] []
                  , use [ y "391", xlinkHref "#g70" ] []
                  , use [ y "408", xlinkHref "#g71" ] []
                  , use [ y "442", xlinkHref "#g72" ] []
                  , use [ y "459", xlinkHref "#g73" ] []
                  , use [ y "476", xlinkHref "#g74" ] []
                  , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "30344ms; anim_last.end+30344ms"
                            , dur "26ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground"
                         , height "17"
                         , width "8"
                         , x "0"
                         , y "493"
                         ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "30370ms; anim_last.end+30370ms"
                            , dur "8ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g49" ] []
                  , use [ y "17", xlinkHref "#g50" ] []
                  , use [ y "34", xlinkHref "#g51" ] []
                  , use [ y "51", xlinkHref "#g52" ] []
                  , use [ y "68", xlinkHref "#g53" ] []
                  , use [ y "102", xlinkHref "#g54" ] []
                  , use [ y "119", xlinkHref "#g19" ] []
                  , use [ y "136", xlinkHref "#g37" ] []
                  , use [ y "153", xlinkHref "#g55" ] []
                  , use [ y "170", xlinkHref "#g56" ] []
                  , use [ y "187", xlinkHref "#g57" ] []
                  , use [ y "204", xlinkHref "#g58" ] []
                  , use [ y "221", xlinkHref "#g19" ] []
                  , use [ y "238", xlinkHref "#g37" ] []
                  , use [ y "255", xlinkHref "#g60" ] []
                  , use [ y "272", xlinkHref "#g19" ] []
                  , use [ y "289", xlinkHref "#g37" ] []
                  , use [ y "306", xlinkHref "#g68" ] []
                  , use [ y "323", xlinkHref "#g69" ] []
                  , use [ y "340", xlinkHref "#g70" ] []
                  , use [ y "357", xlinkHref "#g71" ] []
                  , use [ y "391", xlinkHref "#g72" ] []
                  , use [ y "408", xlinkHref "#g73" ] []
                  , use [ y "425", xlinkHref "#g74" ] []
                  , use [ y "442", xlinkHref "#g75" ] []
                  , use [ y "476", xlinkHref "#g76" ] []
                  , animate [ attributeName "display"
                            , begin "30370ms; anim_last.end+30370ms"
                            , dur "14ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "30378ms; anim_last.end+30378ms"
                            , dur "6ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "16", y "493" ] []
                  , use [ y "493", xlinkHref "#g2" ] []
                  , animate [ attributeName "display"
                            , begin "30384ms; anim_last.end+30384ms"
                            , dur "957ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "24", y "493" ] []
                  , use [ y "493", xlinkHref "#g28" ] []
                  , animate [ attributeName "display"
                            , begin "31341ms; anim_last.end+31341ms"
                            , dur "216ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "32", y "493" ] []
                  , use [ y "493", xlinkHref "#g29" ] []
                  , animate [ attributeName "display"
                            , begin "31557ms; anim_last.end+31557ms"
                            , dur "231ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "40", y "493" ] []
                  , use [ y "493", xlinkHref "#g30" ] []
                  , animate [ attributeName "display"
                            , begin "31788ms; anim_last.end+31788ms"
                            , dur "101ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "48", y "493" ] []
                  , use [ y "493", xlinkHref "#g31" ] []
                  , animate [ attributeName "display"
                            , begin "31889ms; anim_last.end+31889ms"
                            , dur "341ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "56", y "493" ] []
                  , use [ y "493", xlinkHref "#g32" ] []
                  , animate [ attributeName "display"
                            , begin "32230ms; anim_last.end+32230ms"
                            , dur "293ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "64", y "493" ] []
                  , use [ y "493", xlinkHref "#g77" ] []
                  , animate [ attributeName "display"
                            , begin "32523ms; anim_last.end+32523ms"
                            , dur "125ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "72", y "493" ] []
                  , use [ y "493", xlinkHref "#g78" ] []
                  , animate [ attributeName "display"
                            , begin "32648ms; anim_last.end+32648ms"
                            , dur "178ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "80", y "493" ] []
                  , use [ y "493", xlinkHref "#g79" ] []
                  , animate [ attributeName "display"
                            , begin "32826ms; anim_last.end+32826ms"
                            , dur "149ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "88", y "493" ] []
                  , use [ y "493", xlinkHref "#g80" ] []
                  , animate [ attributeName "display"
                            , begin "32975ms; anim_last.end+32975ms"
                            , dur "443ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "96", y "493" ] []
                  , use [ y "493", xlinkHref "#g81" ] []
                  , animate [ attributeName "display"
                            , begin "33418ms; anim_last.end+33418ms"
                            , dur "459ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "104", y "493" ] []
                  , use [ y "493", xlinkHref "#g82" ] []
                  , animate [ attributeName "display"
                            , begin "33877ms; anim_last.end+33877ms"
                            , dur "616ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "112", y "493" ] []
                  , use [ y "493", xlinkHref "#g83" ] []
                  , animate [ attributeName "display"
                            , begin "34493ms; anim_last.end+34493ms"
                            , dur "434ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "120", y "493" ] []
                  , use [ y "493", xlinkHref "#g84" ] []
                  , animate [ attributeName "display"
                            , begin "34927ms; anim_last.end+34927ms"
                            , dur "252ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground", height "17", width "8", x "128", y "493" ] []
                  , use [ y "493", xlinkHref "#g85" ] []
                  , animate [ attributeName "display"
                            , begin "35179ms; anim_last.end+35179ms"
                            , dur "160ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g51" ] []
                  , use [ y "17", xlinkHref "#g52" ] []
                  , use [ y "34", xlinkHref "#g53" ] []
                  , use [ y "68", xlinkHref "#g54" ] []
                  , use [ y "85", xlinkHref "#g19" ] []
                  , use [ y "102", xlinkHref "#g37" ] []
                  , use [ y "119", xlinkHref "#g55" ] []
                  , use [ y "136", xlinkHref "#g56" ] []
                  , use [ y "153", xlinkHref "#g57" ] []
                  , use [ y "170", xlinkHref "#g58" ] []
                  , use [ y "187", xlinkHref "#g19" ] []
                  , use [ y "204", xlinkHref "#g37" ] []
                  , use [ y "221", xlinkHref "#g60" ] []
                  , use [ y "238", xlinkHref "#g19" ] []
                  , use [ y "255", xlinkHref "#g37" ] []
                  , use [ y "272", xlinkHref "#g68" ] []
                  , use [ y "289", xlinkHref "#g69" ] []
                  , use [ y "306", xlinkHref "#g70" ] []
                  , use [ y "323", xlinkHref "#g71" ] []
                  , use [ y "357", xlinkHref "#g72" ] []
                  , use [ y "374", xlinkHref "#g73" ] []
                  , use [ y "391", xlinkHref "#g74" ] []
                  , use [ y "408", xlinkHref "#g75" ] []
                  , use [ y "442", xlinkHref "#g76" ] []
                  , use [ y "459", xlinkHref "#g19" ] []
                  , use [ y "476", xlinkHref "#g86" ] []
                  , animate [ attributeName "display"
                            , begin "30384ms; anim_last.end+30384ms"
                            , dur "4956ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground"
                         , height "17"
                         , width "8"
                         , x "128"
                         , y "493"
                         ] []
                  , use [ y "493", xlinkHref "#g85" ] []
                  , animate [ attributeName "display"
                            , begin "35339ms; anim_last.end+35339ms"
                            , dur "1ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g52" ] []
                  , use [ y "17", xlinkHref "#g53" ] []
                  , use [ y "51", xlinkHref "#g54" ] []
                  , use [ y "68", xlinkHref "#g19" ] []
                  , use [ y "85", xlinkHref "#g37" ] []
                  , use [ y "102", xlinkHref "#g55" ] []
                  , use [ y "119", xlinkHref "#g56" ] []
                  , use [ y "136", xlinkHref "#g57" ] []
                  , use [ y "153", xlinkHref "#g58" ] []
                  , use [ y "170", xlinkHref "#g19" ] []
                  , use [ y "187", xlinkHref "#g37" ] []
                  , use [ y "204", xlinkHref "#g60" ] []
                  , use [ y "221", xlinkHref "#g19" ] []
                  , use [ y "238", xlinkHref "#g37" ] []
                  , use [ y "255", xlinkHref "#g68" ] []
                  , use [ y "272", xlinkHref "#g69" ] []
                  , use [ y "289", xlinkHref "#g70" ] []
                  , use [ y "306", xlinkHref "#g71" ] []
                  , use [ y "340", xlinkHref "#g72" ] []
                  , use [ y "357", xlinkHref "#g73" ] []
                  , use [ y "374", xlinkHref "#g74" ] []
                  , use [ y "391", xlinkHref "#g75" ] []
                  , use [ y "425", xlinkHref "#g76" ] []
                  , use [ y "442", xlinkHref "#g19" ] []
                  , use [ y "459", xlinkHref "#g86" ] []
                  , use [ y "476", xlinkHref "#g87" ] []
                  , rect [ class "foreground"
                         , height "17"
                         , width "8"
                         , x "0"
                         , y "493"
                         ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate
                        [ attributeName "display"
                        , begin "35340ms; anim_last.end+35340ms"
                        , dur "5ms"
                        , from "inline"
                        , to "inline"
                        ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground"
                         , height "17"
                         , width "8"
                         , x "0"
                         , y "493"
                         ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate
                        [ attributeName "display"
                        , begin "35345ms; anim_last.end+35345ms"
                        , dur "8ms"
                        , from "inline"
                        , to "inline"
                        ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g56" ] []
                  , use [ y "17", xlinkHref "#g57" ] []
                  , use [ y "34", xlinkHref "#g58" ] []
                  , use [ y "51", xlinkHref "#g19" ] []
                  , use [ y "68", xlinkHref "#g37" ] []
                  , use [ y "85", xlinkHref "#g60" ] []
                  , use [ y "102", xlinkHref "#g19" ] []
                  , use [ y "119", xlinkHref "#g37" ] []
                  , use [ y "136", xlinkHref "#g68" ] []
                  , use [ y "153", xlinkHref "#g69" ] []
                  , use [ y "170", xlinkHref "#g70" ] []
                  , use [ y "187", xlinkHref "#g71" ] []
                  , use [ y "221", xlinkHref "#g72" ] []
                  , use [ y "238", xlinkHref "#g73" ] []
                  , use [ y "255", xlinkHref "#g74" ] []
                  , use [ y "272", xlinkHref "#g75" ] []
                  , use [ y "306", xlinkHref "#g76" ] []
                  , use [ y "323", xlinkHref "#g19" ] []
                  , use [ y "340", xlinkHref "#g86" ] []
                  , use [ y "357", xlinkHref "#g87" ] []
                  , use [ y "374", xlinkHref "#g88" ] []
                  , use [ y "391", xlinkHref "#g89" ] []
                  , use [ y "408", xlinkHref "#g90" ] []
                  , use [ y "425", xlinkHref "#g91" ] []
                  , use [ y "442", xlinkHref "#g92" ] []
                  , use [ y "476", xlinkHref "#g93" ] []
                  , animate [ attributeName "display"
                            , begin "35345ms; anim_last.end+35345ms"
                            , dur "14ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ rect [ class "foreground"
                         , height "17"
                         , width "8"
                         , x "0"
                         , y "493"
                         ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "35353ms; anim_last.end+35353ms"
                            , dur "6ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g58" ] []
                  , use [ y "17", xlinkHref "#g19" ] []
                  , use [ y "34", xlinkHref "#g37" ] []
                  , use [ y "51", xlinkHref "#g60" ] []
                  , use [ y "68", xlinkHref "#g19" ] []
                  , use [ y "85", xlinkHref "#g37" ] []
                  , use [ y "102", xlinkHref "#g68" ] []
                  , use [ y "119", xlinkHref "#g69" ] []
                  , use [ y "136", xlinkHref "#g70" ] []
                  , use [ y "153", xlinkHref "#g71" ] []
                  , use [ y "187", xlinkHref "#g72" ] []
                  , use [ y "204", xlinkHref "#g73" ] []
                  , use [ y "221", xlinkHref "#g74" ] []
                  , use [ y "238", xlinkHref "#g75" ] []
                  , use [ y "272", xlinkHref "#g76" ] []
                  , use [ y "289", xlinkHref "#g19" ] []
                  , use [ y "306", xlinkHref "#g86" ] []
                  , use [ y "323", xlinkHref "#g87" ] []
                  , use [ y "340", xlinkHref "#g88" ] []
                  , use [ y "357", xlinkHref "#g89" ] []
                  , use [ y "374", xlinkHref "#g90" ] []
                  , use [ y "391", xlinkHref "#g91" ] []
                  , use [ y "408", xlinkHref "#g92" ] []
                  , use [ y "442", xlinkHref "#g93" ] []
                  , use [ y "459", xlinkHref "#g19" ] []
                  , use [ y "476", xlinkHref "#g37" ] []
                  , rect [ class "foreground", height "17", width "8", x "16", y "493" ] []
                  , use [ y "493", xlinkHref "#g2" ] []
                  , animate [ attributeName "display"
                            , begin "35359ms; anim_last.end+35359ms"
                            , dur "1587ms"
                            , from "inline"
                            , to "inline"
                            ] []
                  ]
              , g [ display "none" ]
                  [ use [ y "0", xlinkHref "#g19" ] []
                  , use [ y "17", xlinkHref "#g37" ] []
                  , use [ y "34", xlinkHref "#g60" ] []
                  , use [ y "51", xlinkHref "#g19" ] []
                  , use [ y "68", xlinkHref "#g37" ] []
                  , use [ y "85", xlinkHref "#g68" ] []
                  , use [ y "102", xlinkHref "#g69" ] []
                  , use [ y "119", xlinkHref "#g70" ] []
                  , use [ y "136", xlinkHref "#g71" ] []
                  , use [ y "170", xlinkHref "#g72" ] []
                  , use [ y "187", xlinkHref "#g73" ] []
                  , use [ y "204", xlinkHref "#g74" ] []
                  , use [ y "221", xlinkHref "#g75" ] []
                  , use [ y "255", xlinkHref "#g76" ] []
                  , use [ y "272", xlinkHref "#g19" ] []
                  , use [ y "289", xlinkHref "#g86" ] []
                  , use [ y "306", xlinkHref "#g87" ] []
                  , use [ y "323", xlinkHref "#g88" ] []
                  , use [ y "340", xlinkHref "#g89" ] []
                  , use [ y "357", xlinkHref "#g90" ] []
                  , use [ y "374", xlinkHref "#g91" ] []
                  , use [ y "391", xlinkHref "#g92" ] []
                  , use [ y "425", xlinkHref "#g93" ] []
                  , use [ y "442", xlinkHref "#g19" ] []
                  , use [ y "459", xlinkHref "#g37" ] []
                  , use [ y "476", xlinkHref "#g94" ] []
                  , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
                  , use [ y "493", xlinkHref "#g1" ] []
                  , animate [ attributeName "display"
                            , begin "36946ms; anim_last.end+36946ms"
                            , dur "1000ms"
                            , from "inline"
                            , to "inline"
                            , id "anim_last"
                            ] []
                  ]
              ]
        ]
