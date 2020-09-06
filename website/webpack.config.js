const MiniCssExtractPlugin = require("mini-css-extract-plugin");
const OptimizeCSSAssetsPlugin = require('optimize-css-assets-webpack-plugin');
const CopyPlugin = require('copy-webpack-plugin');
const AutoPrefixer = require('autoprefixer');

module.exports = (env, argv) => ({
  optimization: {
    minimizer: [
      new OptimizeCSSAssetsPlugin({}),
    ]
  },

  entry: {
    index: ['./app.js'],
  },

  output: {
    path: `${__dirname}/dist/js`,
    filename: 'app.js',
  },

  plugins: [
    new MiniCssExtractPlugin({
      filename: '../css/style.css',
    }),
    new CopyPlugin({
      patterns: [
        { from: 'assets/', to: '../' },
        { from: 'index.html', to: '../' },
      ],
    }),
  ],

  module: {
    rules: [
      {
        test: /\.scss$/,
        exclude: [/node_modules/],
        use: [
          {
            loader: MiniCssExtractPlugin.loader,
          },
          {
            loader: "css-loader",
            options: {
              url: false,
              sourceMap: argv.mode === 'development',

              // 0 => no loaders (default);
              // 1 => postcss-loader;
              // 2 => postcss-loader, sass-loader
              importLoaders: 2,
            },
          },
          {
            loader: "postcss-loader",
            options: {
              plugins: [
                AutoPrefixer({ grid: true })
              ],
            },
          },
          {
            loader: "sass-loader",
            options: {
              sourceMap: argv.mode === 'development',
            },
          },
        ],
      },
      {
        test: /\.elm$/,
        exclude: [/elm-stuff/, /node_modules/],
        loader: "elm-webpack-loader",
      },
    ],
    noParse: [/\.elm$/],
  },
});
